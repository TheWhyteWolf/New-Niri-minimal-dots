// SPDX-License-Identifier: GPL-3.0-or-later
// The rain itself: the shader in shaders/rain.frag, the per-output textures it
// reads (a Plate: wallpaper, frosted wallpaper, text), and the per-frame state
// the lock and greeter drive.

use crate::background::Background;
use crate::gpu::Gpu;
use crate::text::Band;
use glow::HasContext;

const FRAG: &str = include_str!("shaders/rain.frag");

/// How many keystroke drops are on the glass at once; the oldest is reused.
pub const IMPACTS: usize = 8;

/// One keystroke drop.
#[derive(Clone, Copy, Debug)]
pub struct Impact {
    /// Where, as fractions of the output (y down).
    pub x: f32,
    pub y: f32,
    /// Rain-clock time it landed; negative = empty slot.
    pub landed: f32,
    /// Rain-clock time Backspace wiped it; negative = still there.
    pub wiped: f32,
}

impl Impact {
    pub const NONE: Impact = Impact { x: 0.0, y: 0.0, landed: -1.0, wiped: -1.0 };
}

/// Everything about one frame that changes over time.
#[derive(Clone, Debug)]
pub struct Frame {
    /// The rain clock in seconds. Callers slow it down (or stop it) rather
    /// than the shader knowing about states.
    pub time: f32,
    /// 0 = clear glass, 1 = fully fogged between the drops.
    pub fog: f32,
    /// 0..1, the share of cells holding a drop.
    pub density: f32,
    pub dim: f32,
    pub vignette: f32,
    /// rgb + amount, mixed over everything (under the text).
    pub tint: [f32; 4],
    pub impacts: [Impact; IMPACTS],
}

impl Default for Frame {
    fn default() -> Frame {
        Frame {
            time: 0.0,
            fog: 1.0,
            density: 0.7,
            dim: 0.85,
            vignette: 0.5,
            tint: [0.0; 4],
            impacts: [Impact::NONE; IMPACTS],
        }
    }
}

/// The shader program, shared by every output.
pub struct Rain {
    program: glow::Program,
}

/// One output's textures, at that output's buffer size.
pub struct Plate {
    pub w: i32,
    pub h: i32,
    sharp: glow::Texture,
    frost: glow::Texture,
    text: Option<(glow::Texture, [f32; 4])>,
}

impl Plate {
    pub fn new(gpu: &Gpu, bg: &Background) -> Result<Plate, String> {
        Ok(Plate {
            w: bg.w as i32,
            h: bg.h as i32,
            sharp: gpu.texture(glow::RGB, bg.w, bg.h, Some(&bg.sharp))?,
            frost: gpu.texture(glow::RGB, bg.frost_w, bg.frost_h, Some(&bg.frost))?,
            text: None,
        })
    }

    /// Replace the text band (None: no text).
    pub fn set_text(&mut self, gpu: &Gpu, band: Option<&Band>) -> Result<(), String> {
        if let Some((t, _)) = self.text.take() {
            gpu.free_texture(t);
        }
        if let Some(b) = band {
            let tex = gpu.texture(glow::RGBA, b.w, b.h, Some(&b.rgba))?;
            self.text = Some((tex, [b.x as f32, b.y as f32, b.w as f32, b.h as f32]));
        }
        Ok(())
    }

    pub fn free(mut self, gpu: &Gpu) {
        if let Some((t, _)) = self.text.take() {
            gpu.free_texture(t);
        }
        gpu.free_texture(self.sharp);
        gpu.free_texture(self.frost);
    }
}

impl Rain {
    pub fn new(gpu: &Gpu) -> Result<Rain, String> {
        Ok(Rain { program: gpu.program(FRAG)? })
    }

    /// Draw one frame of `plate` into whatever is bound.
    pub fn draw(&self, gpu: &Gpu, plate: &Plate, f: &Frame) {
        let gl = &gpu.gl;
        unsafe {
            gl.use_program(Some(self.program));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(plate.sharp));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(plate.frost));
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, plate.text.map(|t| t.0));
            let u = |n: &str| gl.get_uniform_location(self.program, n);
            gl.uniform_1_i32(u("u_sharp").as_ref(), 0);
            gl.uniform_1_i32(u("u_frost").as_ref(), 1);
            gl.uniform_1_i32(u("u_text").as_ref(), 2);
            gl.uniform_2_f32(u("u_res").as_ref(), plate.w as f32, plate.h as f32);
            gl.uniform_1_f32(u("u_time").as_ref(), f.time);
            gl.uniform_1_f32(u("u_fog").as_ref(), f.fog);
            gl.uniform_1_f32(u("u_density").as_ref(), f.density);
            gl.uniform_1_f32(u("u_dim").as_ref(), f.dim);
            gl.uniform_1_f32(u("u_vignette").as_ref(), f.vignette);
            gl.uniform_4_f32_slice(u("u_tint").as_ref(), &f.tint);
            let flat: Vec<f32> = f.impacts.iter().flat_map(|i| [i.x, i.y, i.landed, i.wiped]).collect();
            gl.uniform_4_f32_slice(u("u_impact").as_ref(), &flat);
            gl.uniform_1_f32(u("u_has_text").as_ref(), if plate.text.is_some() { 1.0 } else { 0.0 });
            gl.uniform_4_f32_slice(u("u_text_rect").as_ref(), &plate.text.map_or([0.0, 0.0, 1.0, 1.0], |t| t.1));
        }
        gpu.draw_quad(plate.w, plate.h);
    }

    pub fn free(self, gpu: &Gpu) {
        unsafe { gpu.gl.delete_program(self.program) };
    }
}
