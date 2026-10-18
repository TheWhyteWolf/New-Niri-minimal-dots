// SPDX-License-Identifier: GPL-3.0-or-later
// Wayland greeter application: one xdg-toplevel (cage fullscreens it), the
// event loop, keyboard routing by greeter phase, and the greetd IPC channel.
//
// This is LifeBranch's lifegreet app.rs with the drawing replaced: frames come
// from rainglass on the GPU (shared with rainlock), or as one still frame on
// the CPU when there is no GPU. What the glass does:
//   * typing the username shows it, with a caret, under the clock;
//   * each password keystroke lands a drop; Backspace wipes the newest;
//   * while greetd checks the password the rain slows to a crawl;
//   * a wrong password flashes the glass red, then it's back to the username;
//   * the right one wipes the fog away while the session starts.

use crate::cli::{self, Cfg};
use crate::input::{key_action, Action, PASSWORD_CLEAR_SECS};
use crate::ipc::{Ipc, IpcCmd, IpcEvent};
use crate::secure_buf::SecureBuf;
use crate::sessions::Sessions;
use crate::state::{Greeter, Phase, IDLE_COLLAPSE_SECS, USERNAME_MAX};
use rainglass::{
    buffer_px, text, Background, Band, Font, Frame, Gpu, Impact, Line, Plate, Rain, ScaleHandler,
    Scaling, Source, SurfaceScale, Target, IMPACTS,
};

use smithay_client_toolkit::reexports::client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle,
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            channel::{self, Channel, Event as ChannelEvent},
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        xdg::{
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
            XdgShell,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

/// Size to draw at before the first real configure (dev runs in a floating
/// window; under cage the configure always carries the output size).
const FALLBACK_SIZE: (u32, u32) = (1280, 800);
/// Transient PAM info/error lines stay up this long.
const INFO_SECS: f64 = 6.0;
/// How long to wait for a frame callback before assuming it was lost and
/// repainting anyway (monitors that drop the connector waking from standby).
const FRAME_WATCHDOG: Duration = Duration::from_millis(250);
/// The right password wipes the fog away over this long.
const START_WIPE: f64 = 0.4;
/// How long a wrong password's red flash takes to fade.
const WRONG_FLASH: f64 = 0.9;
/// The glass while the wallpaper is still decoding, or if there is none.
const PLACEHOLDER: [u8; 3] = [29, 32, 39];
const SOURCE_MAX: (u32, u32) = (3840, 2160);

pub struct App {
    loop_handle: LoopHandle<'static, Self>,
    output_state: OutputState,
    registry_state: RegistryState,
    seat_state: SeatState,
    shm: Shm,
    qh: QueueHandle<Self>,

    // Drawing state first, so it is released before the window it draws to.
    plate: Option<Plate>,
    target: Option<Target>,
    pool: Option<SlotPool>,
    cpu_bg: Option<Background>,
    scale: SurfaceScale,
    window: Window,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// Logical size, from configure.
    width: u32,
    height: u32,
    scale120: u32,
    has_wallpaper: bool,
    text_key: String,
    /// Drawn on the CPU (no GPU, or no EGL surface for this window).
    cpu: bool,
    frame_pending: bool,
    frame_pending_since: Option<Instant>,
    watchdog_tripped: bool,
    configured: bool,

    rain: Option<Rain>,
    gpu: Option<Gpu>,
    font: Option<Font>,
    source: Option<Source>,

    cfg: Cfg,
    start: Instant,
    rain_t: f64,
    speed: f64,
    last_tick: Instant,
    impacts: [Impact; IMPACTS],
    next_impact: usize,
    rng: u64,
    wrong_at: Option<f64>,
    started_at: Option<f64>,
    caps_lock: bool,

    greeter: Greeter,
    /// The phase at the previous tick, to catch the collapse back to the
    /// username prompt.
    last_phase: Phase,
    remembered_user: Option<String>,
    sessions: Sessions,
    password: SecureBuf,
    ipc: Ipc,

    /// (message, shown_at) — PAM Info/Error line.
    info: Option<(String, f64)>,
    /// Unrecoverable IPC failure: shown permanently, greeter stays up so the
    /// VT-switch escape hatch remains reachable.
    fatal: Option<String>,

    mods: Modifiers,
    last_key_t: f64,
    exit: bool,
    exit_code: i32,
    password_clear_gen: u64,
}

fn load_font(path: &str) -> Option<Font> {
    let regular = path.replace("-Light.", "-Regular.");
    for p in [path, regular.as_str()] {
        match Font::from_path(p) {
            Ok(f) => return Some(f),
            Err(e) => eprintln!("raingreet: font {p}: {e}"),
        }
    }
    None
}

/// Run the greeter. `ipc_events` is the calloop side of the worker channel.
/// Returns the process exit code (0 = session started; greetd takes over).
pub fn run(cfg: Cfg, ipc: Ipc, ipc_events: Channel<IpcEvent>) -> i32 {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("raingreet: cannot connect to Wayland: {e}");
            return 1;
        }
    };
    let (globals, event_queue) = match registry_queue_init(&conn) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("raingreet: registry init failed: {e}");
            return 1;
        }
    };
    let qh: QueueHandle<App> = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().expect("failed to create event loop");

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let xdg_shell = XdgShell::bind(&globals, &qh).expect("xdg_wm_base");
    let scaling = Scaling::bind(&globals, &qh);
    let surface = compositor.create_surface(&qh);
    let scale = scaling.attach(&surface, &qh);
    let window = xdg_shell.create_window(surface, WindowDecorations::RequestServer, &qh);
    window.set_title("raingreet");
    window.set_app_id("raingreet");
    window.commit();

    let gpu = match Gpu::new(conn.backend().display_ptr().cast()) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("raingreet: no GPU rendering ({e}); drawing a still frame on the CPU");
            None
        }
    };
    let rain = gpu.as_ref().and_then(|g| {
        Rain::new(g)
            .map_err(|e| eprintln!("raingreet: rain shader: {e}"))
            .ok()
    });
    let gpu = if rain.is_some() { gpu } else { None };

    let mut sessions = Sessions::load(&cfg.sessions_dirs, &cfg.cmd);
    let (remembered_user, remembered_session) = cli::recall(&cfg.remember);
    if let Some(s) = &remembered_session {
        sessions.select(s);
    }
    let mut greeter = Greeter::new();
    if let Some(u) = &remembered_user {
        greeter.username = u.clone();
    }

    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9e37_79b9, |d| d.as_nanos() as u64);
    let font = load_font(&cfg.font);
    let mut app = App {
        loop_handle: event_loop.handle(),
        output_state: OutputState::new(&globals, &qh),
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        shm: Shm::bind(&globals, &qh).expect("wl_shm"),
        qh: qh.clone(),
        plate: None,
        target: None,
        pool: None,
        cpu_bg: None,
        scale,
        window,
        keyboard: None,
        width: 0,
        height: 0,
        scale120: 120,
        has_wallpaper: false,
        text_key: String::new(),
        cpu: false,
        frame_pending: false,
        frame_pending_since: None,
        watchdog_tripped: false,
        configured: false,
        rain,
        gpu,
        font,
        source: None,
        cfg,
        start: Instant::now(),
        rain_t: 30.0 + (seed % 3600) as f64,
        speed: 1.0,
        last_tick: Instant::now(),
        impacts: [Impact::NONE; IMPACTS],
        next_impact: 0,
        rng: seed | 1,
        wrong_at: None,
        started_at: None,
        caps_lock: false,
        greeter,
        last_phase: Phase::EnterUser,
        remembered_user,
        sessions,
        password: SecureBuf::new(),
        ipc,
        info: None,
        fatal: None,
        mods: Modifiers::default(),
        last_key_t: 0.0,
        exit: false,
        exit_code: 1,
        password_clear_gen: 0,
    };

    app.loop_handle
        .insert_source(ipc_events, |event, _, app: &mut App| {
            if let ChannelEvent::Msg(ev) = event {
                app.on_ipc_event(ev);
            }
        })
        .expect("insert ipc channel");

    // Decode the wallpaper off the event loop; the placeholder colour shows
    // until it's ready.
    let (tx, rx) = channel::channel::<Source>();
    let path = app.cfg.image.clone();
    std::thread::spawn(
        move || match rainglass::load_source(&path, SOURCE_MAX.0, SOURCE_MAX.1) {
            Ok(src) => {
                let _ = tx.send(src);
            }
            Err(e) => eprintln!("raingreet: wallpaper {e}; showing a plain colour"),
        },
    );
    app.loop_handle
        .insert_source(rx, |event, _, app: &mut App| {
            if let ChannelEvent::Msg(src) = event {
                app.source = Some(src);
            }
        })
        .expect("insert wallpaper source");

    let frame = if app.still() {
        Duration::from_millis(250)
    } else {
        Duration::from_secs_f64(1.0 / app.cfg.fps)
    };
    app.loop_handle
        .insert_source(Timer::from_duration(frame), move |_, _, app: &mut App| {
            app.tick();
            TimeoutAction::ToDuration(frame)
        })
        .expect("insert frame timer");

    WaylandSource::new(conn, event_queue)
        .insert(app.loop_handle.clone())
        .expect("insert wayland source");

    while !app.exit {
        if event_loop
            .dispatch(Duration::from_millis(100), &mut app)
            .is_err()
        {
            break;
        }
    }
    app.exit_code
}

/// Local time "21:47".
fn clock_hhmm() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return "--:--".into();
        }
        format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
    }
}

/// Local date "SUN 18 OCT".
fn clock_date() -> String {
    const DAYS: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];
    const MONTHS: [&str; 12] = [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ];
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return String::new();
        }
        let day = DAYS.get(tm.tm_wday as usize).copied().unwrap_or("");
        let mon = MONTHS.get(tm.tm_mon as usize).copied().unwrap_or("");
        format!("{day} {:02} {mon}", tm.tm_mday)
    }
}

/// One line of text: what, how big relative to the clock, colour, opacity.
type Spec = (String, f32, [u8; 3], f32);

fn key_of(lines: &[Spec]) -> String {
    lines
        .iter()
        .map(|(t, s, c, a)| format!("{t}\u{1}{s}\u{1}{c:?}\u{1}{a}"))
        .collect::<Vec<_>>()
        .join("\u{2}")
}

impl App {
    fn t(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    fn still(&self) -> bool {
        self.cfg.still || self.gpu.is_none() || self.cpu
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    fn splash(&mut self) {
        let (x, y) = (0.08 + 0.84 * self.rand(), 0.08 + 0.84 * self.rand());
        self.impacts[self.next_impact] = Impact {
            x,
            y,
            landed: self.rain_t as f32,
            wiped: -1.0,
        };
        self.next_impact = (self.next_impact + 1) % IMPACTS;
    }

    fn wipe_newest(&mut self) {
        let newest = (0..IMPACTS)
            .filter(|&i| self.impacts[i].landed >= 0.0 && self.impacts[i].wiped < 0.0)
            .max_by(|&a, &b| self.impacts[a].landed.total_cmp(&self.impacts[b].landed));
        if let Some(i) = newest {
            self.impacts[i].wiped = self.rain_t as f32;
        }
    }

    fn wipe_all(&mut self) {
        let now = self.rain_t as f32;
        for i in self
            .impacts
            .iter_mut()
            .filter(|i| i.landed >= 0.0 && i.wiped < 0.0)
        {
            i.wiped = now;
        }
    }

    /// The text under the clock, by phase.
    fn lines(&self, t: f64) -> Vec<Spec> {
        let (text, urgent) = (self.cfg.text, self.cfg.urgent);
        let user = &self.greeter.username;
        let caret = if (t % 1.0) < 0.5 { "▏" } else { " " };
        // The grow/collapse curves from lifegreet's phase machine (there they
        // scaled the cube) cross-fade the prompts: the username dims as the
        // password prompt comes up, and back.
        let user_alpha = 0.6 + 0.35 * self.greeter.box_alpha(t) as f32;
        let prompt: Spec = match self.greeter.phase {
            Phase::EnterUser if user.is_empty() => (format!("username{caret}"), 0.24, text, 0.45),
            Phase::EnterUser => (format!("{user}{caret}"), 0.24, text, 0.95),
            _ => (user.clone(), 0.24, text, user_alpha),
        };
        let info = self.info.as_ref().map(|(m, _)| m.clone());
        let status: Spec = if let Some(f) = &self.fatal {
            (f.clone(), 0.17, urgent, 0.95)
        } else {
            match self.greeter.phase {
                Phase::EnterUser => (info.unwrap_or_default(), 0.17, urgent, 0.95),
                Phase::Growing { .. } | Phase::EnterPassword if self.caps_lock => {
                    ("caps lock is on".into(), 0.17, urgent, 0.95)
                }
                Phase::Growing { .. } | Phase::EnterPassword => (
                    "password".into(),
                    0.17,
                    text,
                    0.5 * self.greeter.cube_scale(t) as f32,
                ),
                Phase::Validating => ("checking…".into(), 0.17, text, 0.8),
                Phase::Failed { .. } => (
                    info.unwrap_or_else(|| "wrong password".into()),
                    0.17,
                    urgent,
                    0.95,
                ),
                Phase::Collapsing { .. } => (String::new(), 0.17, text, 0.0),
                Phase::Starting => (
                    format!("starting {}", self.sessions.current().name),
                    0.17,
                    text,
                    0.8,
                ),
            }
        };
        let session: Spec = if self.sessions.len() > 1 && self.greeter.phase != Phase::Starting {
            (
                format!("F2  {}", self.sessions.current().name),
                0.15,
                text,
                0.55,
            )
        } else {
            (String::new(), 0.15, text, 0.0)
        };
        vec![
            (clock_hhmm(), 1.0, text, 0.95),
            (clock_date(), 0.25, text, 0.8),
            prompt,
            status,
            session,
        ]
    }

    fn band(&self, lines: &[Spec], bw: u32, bh: u32) -> Option<Band> {
        let font = self.font.as_ref()?;
        let s = bh as f32 / self.height.max(1) as f32;
        let clock = self.cfg.clock_px * s;
        let lines: Vec<Line> = lines
            .iter()
            .map(|(text, rel, rgb, alpha)| Line {
                text: text.clone(),
                px: clock * rel,
                rgb: *rgb,
                alpha: *alpha,
            })
            .collect();
        text::band(
            font,
            &lines,
            bw as usize,
            (bh as f32 * 0.28) as i32,
            (clock * 0.14) as usize,
        )
    }

    fn glass(&self, t: f64) -> Frame {
        let mut f = Frame {
            time: self.rain_t as f32,
            density: self.cfg.density,
            dim: self.cfg.dim,
            impacts: self.impacts,
            ..Frame::default()
        };
        if let Some(at) = self.wrong_at {
            let p = ((t - at) / WRONG_FLASH) as f32;
            if p < 1.0 {
                let u = self.cfg.urgent;
                f.tint = [
                    u[0] as f32 / 255.0,
                    u[1] as f32 / 255.0,
                    u[2] as f32 / 255.0,
                    0.3 * (1.0 - p) * (1.0 - p),
                ];
            }
        }
        if let Some(at) = self.started_at {
            let p = (((t - at) / START_WIPE) as f32).min(1.0);
            f.fog = 1.0 - p;
            f.dim += (1.0 - f.dim) * p;
        }
        f
    }

    fn buffer_size(&self) -> (u32, u32) {
        if self.scale.fractional() {
            (
                buffer_px(self.width, self.scale120),
                buffer_px(self.height, self.scale120),
            )
        } else {
            let s = (self.scale120 / 120).max(1);
            (self.width * s, self.height * s)
        }
    }

    fn prepare_surface(&self) {
        let surface = self.window.wl_surface();
        if self.scale.fractional() {
            surface.set_buffer_scale(1);
            self.scale.set_destination(self.width, self.height);
        } else {
            surface.set_buffer_scale((self.scale120 / 120).max(1) as i32);
        }
    }

    /// Forget everything sized to the old size or scale.
    fn reset_surface(&mut self) {
        if let (Some(p), Some(gpu)) = (self.plate.take(), self.gpu.as_ref()) {
            p.free(gpu);
        }
        self.target = None;
        self.pool = None;
        self.cpu_bg = None;
        self.has_wallpaper = false;
        self.text_key.clear();
    }

    fn tick(&mut self) {
        if self.exit {
            return;
        }
        let t = self.t();
        self.greeter.advance(t);

        // Back at the username prompt after a failure: offer the remembered
        // user again rather than an empty box.
        if self.greeter.phase == Phase::EnterUser
            && matches!(self.last_phase, Phase::Collapsing { .. })
        {
            if let Some(u) = &self.remembered_user {
                self.greeter.username = u.clone();
            }
        }
        self.last_phase = self.greeter.phase;

        // Idle at the password prompt with nothing typed: back to the
        // username prompt.
        if self.greeter.phase == Phase::EnterPassword
            && self.password.is_empty()
            && t - self.last_key_t > IDLE_COLLAPSE_SECS
        {
            self.greeter.cancel_to_user(t);
            self.ipc.send(IpcCmd::Cancel);
        }
        if let Some((_, since)) = &self.info {
            if t - since > INFO_SECS {
                self.info = None;
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.25);
        self.last_tick = now;
        let target = if self.greeter.phase == Phase::Validating {
            0.15
        } else {
            1.0
        };
        self.speed += (target - self.speed) * (dt * 4.0).min(1.0);
        if !self.cfg.still {
            self.rain_t += dt * self.speed;
        }

        if !self.configured {
            return;
        }
        let lines = self.lines(t);
        let (bw, bh) = self.buffer_size();
        let key = format!("{}@{bw}x{bh}", key_of(&lines));
        if self.still() && self.text_key == key && (self.has_wallpaper || self.source.is_none()) {
            return; // nothing a still frame shows has changed
        }
        if self.frame_pending {
            let stuck = self
                .frame_pending_since
                .is_some_and(|since| since.elapsed() >= FRAME_WATCHDOG);
            if !stuck {
                return;
            }
            if !self.watchdog_tripped {
                self.watchdog_tripped = true;
                eprintln!("raingreet: frame callback lost (DPMS/hotplug?) — repainting");
            }
            self.frame_pending = false;
        }
        self.paint(t, &lines, key);
    }

    fn paint(&mut self, t: f64, lines: &[Spec], key: String) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let t0 = self.cfg.debug_timing.then(Instant::now);
        let drawn = !self.cpu && self.gpu.is_some() && self.paint_gpu(t, lines, &key);
        if !drawn {
            self.paint_cpu(lines, key);
        }
        if let Some(t0) = t0 {
            eprintln!(
                "raingreet: frame {:.2}ms",
                t0.elapsed().as_secs_f64() * 1000.0
            );
        }
    }

    fn paint_gpu(&mut self, t: f64, lines: &[Spec], key: &str) -> bool {
        let frame = self.glass(t);
        let (bw, bh) = self.buffer_size();
        if self.target.is_none() {
            self.prepare_surface();
            let gpu = self.gpu.as_ref().unwrap();
            match gpu.target(
                self.window.wl_surface().id().as_ptr().cast(),
                bw as i32,
                bh as i32,
            ) {
                Ok(tg) => self.target = Some(tg),
                Err(e) => {
                    eprintln!("raingreet: {e}; drawing on the CPU");
                    self.cpu = true;
                    return false;
                }
            }
        }
        let want_wallpaper = self.source.is_some();
        let rebuild = self.plate.is_none() || (want_wallpaper && !self.has_wallpaper);
        let band = (rebuild || self.text_key != key).then(|| self.band(lines, bw, bh));
        let (Some(gpu), Some(rain)) = (self.gpu.as_ref(), self.rain.as_ref()) else {
            return false;
        };
        if let Err(e) = gpu.bind(self.target.as_ref().unwrap()) {
            eprintln!("raingreet: {e}");
            self.cpu = true;
            self.target = None;
            return false;
        }
        if rebuild {
            if let Some(p) = self.plate.take() {
                p.free(gpu);
            }
            let bg = match &self.source {
                Some(src) => Background::from_source(src, bw, bh),
                None => Background::solid(PLACEHOLDER, bw, bh),
            };
            match Plate::new(gpu, &bg) {
                Ok(p) => {
                    self.plate = Some(p);
                    self.has_wallpaper = want_wallpaper;
                }
                Err(e) => {
                    eprintln!("raingreet: {e}; drawing on the CPU");
                    self.cpu = true;
                    self.target = None;
                    return false;
                }
            }
        }
        if let Some(band) = band {
            if let Err(e) = self.plate.as_mut().unwrap().set_text(gpu, band.as_ref()) {
                eprintln!("raingreet: text: {e}");
            }
            self.text_key = key.to_string();
        }
        self.prepare_surface();
        rain.draw(gpu, self.plate.as_ref().unwrap(), &frame);
        let surface = self.window.wl_surface();
        surface.frame(&self.qh, surface.clone());
        if let Err(e) = gpu.present(self.target.as_ref().unwrap()) {
            eprintln!("raingreet: {e}");
            return true;
        }
        self.frame_pending = true;
        self.frame_pending_since = Some(Instant::now());
        true
    }

    fn paint_cpu(&mut self, lines: &[Spec], key: String) {
        let (bw, bh) = self.buffer_size();
        let want_wallpaper = self.source.is_some();
        if self.cpu_bg.is_none() || (want_wallpaper && !self.has_wallpaper) {
            let mut bg = match &self.source {
                Some(src) => Background::from_source(src, bw, bh),
                None => Background::solid(PLACEHOLDER, bw, bh),
            };
            bg.sharp = Vec::new(); // only the frosted copy is drawn
            self.cpu_bg = Some(bg);
            self.has_wallpaper = want_wallpaper;
        }
        let stride = bw as i32 * 4;
        if self.pool.is_none() {
            match SlotPool::new((stride * bh as i32) as usize, &self.shm) {
                Ok(p) => self.pool = Some(p),
                Err(e) => {
                    eprintln!("raingreet: shm pool failed: {e}");
                    return;
                }
            }
        }
        let band = self.band(lines, bw, bh);
        self.prepare_surface();
        let pool = self.pool.as_mut().unwrap();
        match pool.create_buffer(bw as i32, bh as i32, stride, wl_shm::Format::Xrgb8888) {
            Ok((buffer, canvas)) => {
                rainglass::still::paint(
                    canvas,
                    bw as usize,
                    bh as usize,
                    self.cpu_bg.as_ref().unwrap(),
                    band.as_ref(),
                    self.cfg.dim,
                );
                let surface = self.window.wl_surface();
                surface.frame(&self.qh, surface.clone());
                buffer.attach_to(surface).expect("attach");
                surface.damage_buffer(0, 0, bw as i32, bh as i32);
                surface.commit();
                self.text_key = key;
                self.frame_pending = true;
                self.frame_pending_since = Some(Instant::now());
            }
            Err(e) => eprintln!("raingreet: buffer alloc failed: {e}"),
        }
    }

    // ----- keyboard -----------------------------------------------------------

    fn on_key(&mut self, event: KeyEvent) {
        if self.exit {
            return;
        }
        let t = self.t();
        self.last_key_t = t;
        let ctrl = self.mods.ctrl;

        // Global: Ctrl+Alt+Del reboots (the greeter user may command the
        // active seat via logind, as tuigreet's power option does).
        if ctrl && self.mods.alt && matches!(event.keysym, Keysym::Delete | Keysym::KP_Delete) {
            let _ = std::process::Command::new("systemctl")
                .arg("reboot")
                .spawn();
            return;
        }

        match self.greeter.phase {
            Phase::EnterUser => self.on_key_username(event, ctrl, t),
            Phase::Growing { .. } | Phase::EnterPassword => self.on_key_password(event, ctrl, t),
            // Validating/Failed/Collapsing/Starting: input is ignored.
            _ => {}
        }
    }

    /// Username entry: visible text, plain editing. F2 (or F3) cycles sessions.
    fn on_key_username(&mut self, event: KeyEvent, ctrl: bool, t: f64) {
        match event.keysym {
            Keysym::F2 | Keysym::F3 => self.sessions.cycle(),
            Keysym::Return | Keysym::KP_Enter => {
                if self.greeter.user_submit(t) {
                    self.ipc.send(IpcCmd::CreateSession {
                        username: self.greeter.username.trim().to_string(),
                    });
                }
            }
            Keysym::BackSpace => {
                if ctrl {
                    self.greeter.username.clear();
                } else {
                    self.greeter.username.pop();
                }
            }
            Keysym::Escape => self.greeter.username.clear(),
            Keysym::u | Keysym::U if ctrl => self.greeter.username.clear(),
            _ => {
                if ctrl {
                    return;
                }
                let Some(s) = event.utf8.as_deref() else {
                    return;
                };
                for c in s.chars() {
                    if !c.is_control() && self.greeter.username.chars().count() < USERNAME_MAX {
                        self.greeter.username.push(c);
                    }
                }
            }
        }
    }

    /// Password entry: swaylock semantics on the SecureBuf; drops only.
    fn on_key_password(&mut self, event: KeyEvent, ctrl: bool, t: f64) {
        match key_action(event.keysym, event.utf8.as_deref(), ctrl) {
            Action::Append(mut s) => {
                self.password.push_str(&s);
                // Wipe this transient heap copy once it's in the mlock'd
                // buffer (the KeyEvent's own String is dropped by sctk).
                s.zeroize();
                self.splash();
                self.arm_password_clear();
            }
            Action::PopChar => {
                if self.password.pop_char() {
                    if self.password.is_empty() {
                        self.wipe_all();
                    } else {
                        self.wipe_newest();
                        self.arm_password_clear();
                    }
                }
            }
            Action::ClearAll => {
                if event.keysym == Keysym::Escape && self.password.is_empty() {
                    // Username-typo recovery: back out to the username prompt.
                    self.greeter.cancel_to_user(t);
                    self.ipc.send(IpcCmd::Cancel);
                }
                self.password.clear();
                self.wipe_all();
            }
            Action::Submit => {
                if self.password.is_empty() {
                    return; // ignore-empty-password
                }
                if self.greeter.password_submit() {
                    // Transient copy for the wire; the worker zeroizes it
                    // after serialization (see ipc.rs).
                    let pw = String::from_utf8_lossy(self.password.as_bytes()).into_owned();
                    self.password.clear();
                    self.ipc.send(IpcCmd::PostAuth { password: Some(pw) });
                }
            }
            Action::Neutral | Action::Ignore => {}
        }
    }

    // ----- greetd events ------------------------------------------------------

    fn failed(&mut self, t: f64) {
        self.password.clear();
        self.wipe_all();
        self.wrong_at = Some(t);
        self.greeter.on_auth_failed(t);
        self.ipc.send(IpcCmd::Cancel);
    }

    fn on_ipc_event(&mut self, ev: IpcEvent) {
        let t = self.t();
        match ev {
            IpcEvent::Prompt { secret, message } => {
                // The first (password) prompt is expected — entry is already
                // open. A prompt arriving while Validating means greetd wants
                // a SECOND answer (e.g. 2FA), which this UI can't take.
                if self.greeter.phase == Phase::Validating {
                    eprintln!("raingreet: unsupported extra prompt (secret={secret}): {message}");
                    self.failed(t);
                    self.info = Some(("unsupported extra auth prompt".into(), t));
                }
            }
            IpcEvent::Info(m) => self.info = Some((m, t)),
            IpcEvent::AuthOk => {
                if self.greeter.on_auth_ok() {
                    cli::remember(
                        &self.cfg.remember,
                        self.greeter.username.trim(),
                        &self.sessions.current().name,
                    );
                    self.started_at = Some(t);
                    // systemd-cat: greetd gives the session vt1 as stdio, so
                    // its startup chatter would sit in the VT text buffer and
                    // flash as terminal text during the KMS handoff. Journal
                    // instead: journalctl -t greetd-session
                    let mut cmd: Vec<String> = ["systemd-cat", "-t", "greetd-session", "--"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    cmd.extend(self.sessions.current().cmd.iter().cloned());
                    self.ipc.send(IpcCmd::StartSession { cmd });
                }
            }
            IpcEvent::AuthFailed(m) => {
                eprintln!("raingreet: {m}");
                self.failed(t);
            }
            IpcEvent::SessionStarted => {
                // greetd launches the session once we exit; cage follows.
                self.exit_code = 0;
                self.exit = true;
            }
            IpcEvent::Fatal(m) => {
                eprintln!("raingreet: fatal: {m}");
                self.fatal = Some(m);
            }
        }
    }

    // ----- timers -------------------------------------------------------------

    fn arm_password_clear(&mut self) {
        self.password_clear_gen += 1;
        let gen = self.password_clear_gen;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_secs_f64(PASSWORD_CLEAR_SECS)),
            move |_, _, app: &mut App| {
                if app.password_clear_gen == gen && !app.password.is_empty() {
                    app.password.clear();
                    app.wipe_all();
                }
                TimeoutAction::Drop
            },
        );
    }
}

impl WindowHandler for App {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        // Never happens under cage; honoring it in dev runs is convenient.
        self.exit_code = 1;
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let w = configure
            .new_size
            .0
            .map(|v| v.get())
            .unwrap_or(FALLBACK_SIZE.0);
        let h = configure
            .new_size
            .1
            .map(|v| v.get())
            .unwrap_or(FALLBACK_SIZE.1);
        if w != self.width || h != self.height {
            self.reset_surface();
            self.width = w;
            self.height = h;
        }
        self.configured = true;
        self.frame_pending = false;
        self.frame_pending_since = None;
        self.watchdog_tripped = false;
        let t = self.t();
        let lines = self.lines(t);
        let (bw, bh) = self.buffer_size();
        let key = format!("{}@{bw}x{bh}", key_of(&lines));
        self.paint(t, &lines, key);
    }
}

impl ScaleHandler for App {
    fn preferred_scale(&mut self, _surface: &wl_surface::WlSurface, scale120: u32) {
        if self.scale120 != scale120 {
            self.reset_surface();
            self.scale120 = scale120;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        factor: i32,
    ) {
        let s120 = factor.max(1) as u32 * 120;
        if !self.scale.fractional() && self.scale120 != s120 {
            self.reset_surface();
            self.scale120 = s120;
        }
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _: u32,
    ) {
        if surface == self.window.wl_surface() {
            if self.watchdog_tripped {
                self.watchdog_tripped = false;
                eprintln!("raingreet: frame callbacks resumed");
            }
            self.frame_pending = false;
            self.frame_pending_since = None;
        }
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|app: &mut App, _kbd, event| app.on_key(event)),
            ) {
                Ok(kbd) => self.keyboard = Some(kbd),
                Err(e) => eprintln!("raingreet: no keyboard: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard {
            if let Some(kbd) = self.keyboard.take() {
                kbd.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        self.on_key(event);
    }
    fn repeat_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
        // Compositor-driven (wl_keyboard v10) repeats — no-op. Repeat is driven
        // by the get_keyboard_with_repeat calloop callback, so handling this
        // too would double-type on v10 compositors.
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: KeyEvent,
    ) {
    }
    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        modifiers: Modifiers,
        _: RawModifiers,
        _: u32,
    ) {
        self.mods = modifiers;
        self.caps_lock = modifiers.caps_lock;
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_compositor!(App);
smithay_client_toolkit::delegate_output!(App);
smithay_client_toolkit::delegate_seat!(App);
smithay_client_toolkit::delegate_keyboard!(App);
smithay_client_toolkit::delegate_shm!(App);
smithay_client_toolkit::delegate_xdg_shell!(App);
smithay_client_toolkit::delegate_xdg_window!(App);
smithay_client_toolkit::delegate_registry!(App);
rainglass::delegate_scaling!(App);
