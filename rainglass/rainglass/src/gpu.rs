// SPDX-License-Identifier: GPL-3.0-or-later
// EGL + GLES2 plumbing, adapted from LifeBranch's lifewall (lifewall/src/gpu.rs):
// one context for every surface, a window surface per wl_surface, and an
// offscreen framebuffer for rendering straight to a PNG (`preview --png`).
//
// GLES2 / GLSL ES 1.00 on purpose: it is what every Mesa driver, old Intel
// iGPUs included, can do.

use glow::HasContext;
use khronos_egl as egl;
use std::ffi::c_void;

pub struct Gpu {
    egl: &'static egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    pub gl: glow::Context,
    quad: glow::Buffer,
}

/// One wl_surface's EGL window surface.
pub struct Target {
    window: *mut wayland_sys::egl::wl_egl_window,
    surface: egl::Surface,
    display: egl::Display,
    pub w: i32,
    pub h: i32,
}

impl Drop for Target {
    fn drop(&mut self) {
        let _ = egl::API.destroy_surface(self.display, self.surface);
        // SAFETY: created by wl_egl_window_create, destroyed exactly once,
        // after the EGL surface that used it.
        unsafe { wayland_sys::egl::wl_egl_window_destroy(self.window) };
    }
}

fn err(what: &str) -> impl Fn(egl::Error) -> String + '_ {
    move |e| format!("{what}: {e}")
}

/// Full-screen quad: every draw is one fragment shader over the whole target.
pub const VERT: &str = "attribute vec2 a_pos;
void main() { gl_Position = vec4(a_pos, 0.0, 1.0); }";

impl Gpu {
    /// `display` is the libwayland `wl_display*` of the connection in use.
    pub fn new(display: *mut c_void) -> Result<Gpu, String> {
        let egl = &egl::API;
        // SAFETY: a live wl_display pointer for the whole process lifetime.
        let display = unsafe { egl.get_display(display) }.ok_or("no EGL display")?;
        egl.initialize(display).map_err(err("eglInitialize"))?;
        egl.bind_api(egl::OPENGL_ES_API).map_err(err("eglBindAPI"))?;
        let attrs = [
            egl::SURFACE_TYPE, egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT,
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &attrs)
            .map_err(err("eglChooseConfig"))?
            .ok_or("no EGL config")?;
        let ctx_attrs = [egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE];
        let context = egl.create_context(display, config, None, &ctx_attrs).map_err(err("eglCreateContext"))?;
        // Surfaceless is fine for compiling shaders and for offscreen
        // framebuffers (EGL_KHR_surfaceless_context, which every Mesa driver has).
        egl.make_current(display, None, None, Some(context)).map_err(err("eglMakeCurrent"))?;
        // SAFETY: the loader returns entry points for the context just made current.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name).map_or(std::ptr::null(), |f| f as *const c_void)
            })
        };
        let quad = unsafe { quad(&gl)? };
        Ok(Gpu { egl, display, config, context, gl, quad })
    }

    /// An EGL surface for `surface` (a `wl_surface*`) at `w`x`h` buffer pixels.
    pub fn target(&self, surface: *mut c_void, w: i32, h: i32) -> Result<Target, String> {
        // SAFETY: a live wl_surface proxy; the window is destroyed in Target's Drop.
        let window = unsafe { wayland_sys::egl::wl_egl_window_create(surface.cast(), w, h) };
        if window.is_null() {
            return Err("wl_egl_window_create failed".into());
        }
        // SAFETY: the window was just created and outlives the surface (Drop order).
        let made = unsafe { self.egl.create_window_surface(self.display, self.config, window.cast(), None) };
        let surface = match made {
            Ok(s) => s,
            Err(e) => {
                unsafe { wayland_sys::egl::wl_egl_window_destroy(window) };
                return Err(format!("eglCreateWindowSurface: {e}"));
            }
        };
        let t = Target { window, surface, display: self.display, w, h };
        self.bind(&t)?;
        // Never block in eglSwapBuffers: pacing is the caller's frame callbacks,
        // so a hidden output simply stops being drawn instead of stalling.
        self.egl.swap_interval(self.display, 0).map_err(err("eglSwapInterval"))?;
        Ok(t)
    }

    pub fn bind(&self, t: &Target) -> Result<(), String> {
        self.egl
            .make_current(self.display, Some(t.surface), Some(t.surface), Some(self.context))
            .map_err(err("eglMakeCurrent"))
    }

    /// Back to no surface, for offscreen rendering.
    pub fn unbind(&self) -> Result<(), String> {
        self.egl.make_current(self.display, None, None, Some(self.context)).map_err(err("eglMakeCurrent"))
    }

    pub fn present(&self, t: &Target) -> Result<(), String> {
        self.egl.swap_buffers(self.display, t.surface).map_err(err("eglSwapBuffers"))
    }

    /// Compile and link a full-screen fragment shader against `VERT`.
    pub fn program(&self, frag: &str) -> Result<glow::Program, String> {
        let gl = &self.gl;
        unsafe {
            let program = gl.create_program()?;
            for (kind, src) in [(glow::VERTEX_SHADER, VERT), (glow::FRAGMENT_SHADER, frag)] {
                let sh = gl.create_shader(kind)?;
                gl.shader_source(sh, src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    return Err(format!("shader: {}", gl.get_shader_info_log(sh)));
                }
                gl.attach_shader(program, sh);
            }
            gl.bind_attrib_location(program, 0, "a_pos");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(format!("link: {}", gl.get_program_info_log(program)));
            }
            Ok(program)
        }
    }

    /// An RGB or RGBA texture with linear filtering (no mipmaps: GLES2 can't
    /// mipmap textures whose sides aren't powers of two).
    pub fn texture(&self, format: u32, w: usize, h: usize, data: Option<&[u8]>) -> Result<glow::Texture, String> {
        let gl = &self.gl;
        unsafe {
            let tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D, 0, format as i32, w as i32, h as i32, 0,
                format, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(data),
            );
            Ok(tex)
        }
    }

    pub fn update_texture(&self, tex: glow::Texture, format: u32, w: usize, h: usize, data: &[u8]) {
        let gl = &self.gl;
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_2D, 0, 0, 0, w as i32, h as i32,
                format, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(data)),
            );
        }
    }

    pub fn free_texture(&self, tex: glow::Texture) {
        unsafe { self.gl.delete_texture(tex) };
    }

    /// Draw the full-screen quad with whatever program and uniforms are bound.
    pub fn draw_quad(&self, w: i32, h: i32) {
        let gl = &self.gl;
        unsafe {
            gl.viewport(0, 0, w, h);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.quad));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    /// Render offscreen at `w`x`h` and read the pixels back as top-down RGBA.
    pub fn offscreen(&self, w: i32, h: i32, draw: impl FnOnce()) -> Result<Vec<u8>, String> {
        self.unbind()?;
        let gl = &self.gl;
        unsafe {
            let tex = self.texture(glow::RGBA, w as usize, h as usize, None)?;
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("offscreen framebuffer incomplete".into());
            }
            draw();
            let mut px = vec![0u8; (w * h * 4) as usize];
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut px)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            // GL's rows run bottom-up.
            let row = (w * 4) as usize;
            let flipped: Vec<u8> = px.chunks_exact(row).rev().flatten().copied().collect();
            Ok(flipped)
        }
    }
}

unsafe fn quad(gl: &glow::Context) -> Result<glow::Buffer, String> {
    unsafe {
        let quad = gl.create_buffer()?;
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(quad));
        let verts: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0];
        let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_ne_bytes()).collect();
        gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &bytes, glow::STATIC_DRAW);
        Ok(quad)
    }
}
