// SPDX-License-Identifier: GPL-3.0-or-later
// preview: look at the rain without locking anything.
//
//   cargo run --release --example preview -- --image ~/Pictures/wallpapers/x.jpg --png /tmp/rain.png
//
// Renders one frame offscreen (no window) at --size, --time seconds into the
// rain, and writes it as a PNG. --splash adds three keystroke drops that
// landed just before that moment; --clock draws a sample clock and date.

use rainglass::{Background, Font, Frame, Gpu, Impact, Line, Plate, Rain};
use smithay_client_toolkit::reexports::client::Connection;

struct Args {
    image: Option<String>,
    png: String,
    size: (u32, u32),
    time: f32,
    fog: f32,
    density: f32,
    splash: bool,
    clock: bool,
    bench: u32,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        image: None,
        png: "rain.png".into(),
        size: (1920, 1080),
        time: 6.0,
        fog: 1.0,
        density: 0.7,
        splash: false,
        clock: false,
        bench: 0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--image" => a.image = Some(v()?),
            "--png" => a.png = v()?,
            "--size" => {
                let s = v()?;
                let (w, h) = s.split_once('x').ok_or("--size WxH")?;
                a.size = (w.parse().map_err(|_| "--size WxH")?, h.parse().map_err(|_| "--size WxH")?);
            }
            "--time" => a.time = v()?.parse().map_err(|_| "--time SECONDS")?,
            "--fog" => a.fog = v()?.parse().map_err(|_| "--fog 0..1")?,
            "--density" => a.density = v()?.parse().map_err(|_| "--density 0..1")?,
            "--splash" => a.splash = true,
            "--clock" => a.clock = true,
            "--bench" => a.bench = v()?.parse().map_err(|_| "--bench FRAMES")?,
            _ => return Err(format!("unknown option {k}")),
        }
    }
    Ok(a)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("preview: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let a = parse()?;
    let conn = Connection::connect_to_env().map_err(|e| format!("Wayland: {e}"))?;
    let gpu = Gpu::new(conn.backend().display_ptr().cast())?;
    let (w, h) = a.size;
    let t0 = std::time::Instant::now();
    let bg = match &a.image {
        Some(p) => Background::load(p, w, h)?,
        None => Background::solid([40, 48, 56], w, h),
    };
    eprintln!("preview: background {}x{} in {:?}", w, h, t0.elapsed());
    let rain = Rain::new(&gpu)?;
    let mut plate = Plate::new(&gpu, &bg)?;
    if a.clock {
        let font = Font::from_path("/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Light.ttf")?;
        let s = h as f32 / 1080.0;
        let lines = [
            Line { text: "21:47".into(), px: 150.0 * s, rgb: [236, 239, 244], alpha: 0.95 },
            Line { text: "SUN 18 OCT".into(), px: 30.0 * s, rgb: [216, 222, 233], alpha: 0.8 },
            Line { text: "wrong password".into(), px: 22.0 * s, rgb: [224, 108, 117], alpha: 0.95 },
        ];
        let band = rainglass::text::band(&font, &lines, w as usize, (h as f32 * 0.32) as i32, (18.0 * s) as usize);
        plate.set_text(&gpu, band.as_ref())?;
    }
    let mut f = Frame { time: a.time, fog: a.fog, density: a.density, ..Frame::default() };
    if a.splash {
        for (i, (x, y, ago)) in [(0.3, 0.35, 0.08), (0.62, 0.55, 0.6), (0.45, 0.7, 2.0)].into_iter().enumerate() {
            f.impacts[i] = Impact { x, y, landed: a.time - ago, wiped: -1.0 };
        }
    }
    if a.bench > 0 {
        // Draw a run of frames at advancing times, waiting for the GPU after
        // each, and report the average: what one frame of the lock costs.
        let mut took = std::time::Duration::ZERO;
        gpu.offscreen(w as i32, h as i32, || {
            use glow::HasContext;
            for i in 0..a.bench {
                let g = Frame { time: a.time + i as f32 / 30.0, ..f.clone() };
                let t = std::time::Instant::now();
                rain.draw(&gpu, &plate, &g);
                unsafe { gpu.gl.finish() };
                took += t.elapsed();
            }
        })?;
        let renderer = unsafe { glow::HasContext::get_parameter_string(&gpu.gl, glow::RENDERER) };
        eprintln!("preview: {} frames at {w}x{h} on {renderer}: {:.2} ms each", a.bench, took.as_secs_f64() * 1000.0 / a.bench as f64);
    }
    let t1 = std::time::Instant::now();
    let px = gpu.offscreen(w as i32, h as i32, || rain.draw(&gpu, &plate, &f))?;
    eprintln!("preview: frame in {:?}", t1.elapsed());
    image::save_buffer(&a.png, &px, w, h, image::ExtendedColorType::Rgba8).map_err(|e| format!("{}: {e}", a.png))?;
    eprintln!("preview: wrote {}", a.png);
    plate.free(&gpu);
    rain.free(&gpu);
    Ok(())
}
