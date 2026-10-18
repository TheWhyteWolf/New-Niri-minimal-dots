// SPDX-License-Identifier: GPL-3.0-or-later
// Wayland session-lock application: per-output lock surfaces, keyboard
// handling, the animation timer, and the auth channel.
//
// This is LifeBranch's lifelock app.rs (sctk's session_lock example plus what
// a real locker needs: keyboard input, output hotplug while locked, the
// locked -> ready-fd -> daemonize ordering, the frame-callback watchdog, the
// opt-in SIGUSR1 escape hatch) with only the drawing replaced: frames come
// from rainglass on the GPU, at the output's real pixel size through
// fractional scaling, and as one still frame on the CPU (rainglass::still)
// if there is no GPU.
//
// What the glass does:
//   * each keystroke lands a drop at a random spot (gone in ~2 s);
//   * Backspace wipes the newest drop, Esc / Ctrl+U wipe them all;
//   * while PAM checks the password the rain slows to a crawl;
//   * a wrong password flashes the glass red;
//   * the right one wipes the fog away, then unlocks.

use crate::auth::parent::AuthProc;
use crate::cli::Cfg;
use crate::input::{key_action, Action, AuthState, Machine};
use crate::secure_buf::SecureBuf;
use rainglass::{
    buffer_px, text, Background, Band, Font, Frame, Gpu, Impact, Line, Plate, Rain, ScaleHandler, Scaling, Source,
    SurfaceScale, Target, IMPACTS,
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            channel,
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
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface, SessionLockSurfaceConfigure,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use smithay_client_toolkit::reexports::client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_seat, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle,
};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

/// How long to wait for a frame callback before assuming it was lost and
/// repainting anyway. Callbacks vanish when an output power-cycles under us
/// (some monitors drop the connector when waking from deep standby, so niri
/// destroys and recreates the output several times in a row); without this
/// watchdog one lost callback freezes the picture while input keeps working
/// invisibly. From lifelock, which found it the hard way.
const FRAME_WATCHDOG: Duration = Duration::from_millis(250);

/// The watchdog's ceiling once it is clear nobody is going to answer. A lost
/// callback and a powered-off monitor look the same from here, and the idle
/// chain powers the monitors off five minutes after locking; so the wait
/// doubles on each unanswered repaint up to this cap, and resets the moment a
/// real callback or a keypress says someone is there.
const FRAME_WATCHDOG_MAX: Duration = Duration::from_secs(4);

/// The right password wipes the fog away for this long before unlocking.
const UNLOCK_WIPE: Duration = Duration::from_millis(280);
/// How long a wrong password's red flash takes to fade.
const WRONG_FLASH: Duration = Duration::from_millis(900);
/// The glass while the wallpaper is still decoding, or if there is none.
const PLACEHOLDER: [u8; 3] = [29, 32, 39];
/// The largest a decoded wallpaper is kept: covers a 4K output.
const SOURCE_MAX: (u32, u32) = (3840, 2160);

struct LockOutput {
    // Drawing state first, so it is released before the surface it draws to.
    plate: Option<Plate>,
    target: Option<Target>,
    pool: Option<SlotPool>,
    cpu_bg: Option<Background>,
    scale: SurfaceScale,
    output: wl_output::WlOutput,
    surface: SessionLockSurface,
    /// Logical size, from configure.
    width: u32,
    height: u32,
    /// Preferred scale in 120ths (from wp-fractional-scale, else the
    /// output's integer scale * 120).
    scale120: u32,
    /// The plate (or cpu_bg) shows the wallpaper, not the placeholder.
    has_wallpaper: bool,
    /// What the plate's text band says; re-rendered when it differs.
    text_key: String,
    /// This output is drawn on the CPU (no EGL surface for it).
    cpu: bool,
    frame_pending: bool,
    frame_pending_since: Option<Instant>,
    watchdog_tripped: bool,
    watchdog_wait: Duration,
    configured: bool,
}

/// The wait after another unanswered repaint: double it, but never past the cap.
fn watchdog_backoff(wait: Duration) -> Duration {
    (wait * 2).min(FRAME_WATCHDOG_MAX)
}

impl LockOutput {
    fn new(output: wl_output::WlOutput, surface: SessionLockSurface, scale: SurfaceScale, int_scale: i32) -> LockOutput {
        LockOutput {
            plate: None,
            target: None,
            pool: None,
            cpu_bg: None,
            scale,
            output,
            surface,
            width: 0,
            height: 0,
            scale120: int_scale.max(1) as u32 * 120,
            has_wallpaper: false,
            text_key: String::new(),
            cpu: false,
            frame_pending: false,
            frame_pending_since: None,
            watchdog_tripped: false,
            watchdog_wait: FRAME_WATCHDOG,
            configured: false,
        }
    }

    /// Something proved the output is alive and being watched: go back to
    /// answering at full speed.
    fn reset_watchdog(&mut self) {
        self.watchdog_wait = FRAME_WATCHDOG;
    }

    /// Buffer pixels: the real panel size with fractional scaling, else the
    /// logical size times the integer scale.
    fn buffer_size(&self) -> (u32, u32) {
        if self.scale.fractional() {
            (buffer_px(self.width, self.scale120), buffer_px(self.height, self.scale120))
        } else {
            let s = (self.scale120 / 120).max(1);
            (self.width * s, self.height * s)
        }
    }

    /// Tell the compositor how the next buffer maps onto the surface.
    fn prepare_surface(&self) {
        let surface = self.surface.wl_surface();
        if self.scale.fractional() {
            surface.set_buffer_scale(1);
            self.scale.set_destination(self.width, self.height);
        } else {
            surface.set_buffer_scale((self.scale120 / 120).max(1) as i32);
        }
    }

    /// Forget everything sized to the old size or scale.
    fn reset(&mut self, gpu: Option<&Gpu>) {
        if let (Some(p), Some(gpu)) = (self.plate.take(), gpu) {
            p.free(gpu);
        }
        self.target = None;
        self.pool = None;
        self.cpu_bg = None;
        self.has_wallpaper = false;
        self.text_key.clear();
    }
}

/// What the text band says, independent of any output.
struct Content {
    clock: String,
    date: String,
    status: String,
    status_rgb: [u8; 3],
}

impl Content {
    fn key(&self) -> String {
        format!("{}|{}|{}|{:?}", self.clock, self.date, self.status, self.status_rgb)
    }
}

pub struct App {
    loop_handle: LoopHandle<'static, Self>,
    conn: Connection,
    compositor: CompositorState,
    output_state: OutputState,
    registry_state: RegistryState,
    seat_state: SeatState,
    shm: Shm,
    scaling: Scaling,
    qh: QueueHandle<Self>,

    session_lock_state: SessionLockState,
    session_lock: Option<SessionLock>,
    outputs: Vec<LockOutput>,
    keyboard: Option<wl_keyboard::WlKeyboard>,

    // Drawing. `rain` exists only with `gpu`; without both, outputs are
    // drawn once on the CPU.
    rain: Option<Rain>,
    gpu: Option<Gpu>,
    font: Option<Font>,
    source: Option<Source>,

    cfg: Cfg,
    /// The rain clock (seconds) and how fast it runs right now.
    rain_t: f64,
    speed: f64,
    last_tick: Instant,
    impacts: [Impact; IMPACTS],
    next_impact: usize,
    rng: u64,
    wrong_at: Option<Instant>,
    unlock_at: Option<Instant>,
    /// Something changed that a still (non-animated) output must show.
    dirty: bool,
    caps_lock: bool,
    /// PAM's own words after a failure (faillock lockout etc.).
    status_msg: Option<String>,

    machine: Machine,
    password: SecureBuf,
    auth: AuthProc,

    mods: Modifiers,
    locked: bool,
    saw_first_key: bool,
    exit: bool,
    unlocked_ok: bool,
    final_code: i32,
    auth_dead: bool,
    // Deferred timer tokens (armed as calloop timers keyed by generation).
    input_idle_gen: u64,
    password_clear_gen: u64,
    auth_invalid_gen: u64,
}

fn load_font(path: &str) -> Option<Font> {
    // The configured face, then the regular weight of the same family.
    let regular = path.replace("-Light.", "-Regular.");
    for p in [path, regular.as_str()] {
        match Font::from_path(p) {
            Ok(f) => return Some(f),
            Err(e) => eprintln!("rainlock: font {p}: {e}"),
        }
    }
    eprintln!("rainlock: no font — the lock works, but shows no clock or prompts");
    None
}

/// Run the UI process. `auth` is the already-spawned auth child.
/// Returns the process exit code.
pub fn run(cfg: Cfg, auth: AuthProc) -> i32 {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rainlock: cannot connect to Wayland: {e}");
            return 1;
        }
    };
    let (globals, event_queue) = match registry_queue_init(&conn) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("rainlock: registry init failed: {e}");
            return 1;
        }
    };
    let qh: QueueHandle<App> = event_queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().expect("failed to create event loop");

    // The GPU is optional: without it the lock still covers every output and
    // takes the password, it just doesn't rain.
    let gpu = match Gpu::new(conn.backend().display_ptr().cast()) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("rainlock: no GPU rendering ({e}); drawing a still frame on the CPU");
            None
        }
    };
    let rain = gpu.as_ref().and_then(|g| Rain::new(g).map_err(|e| eprintln!("rainlock: rain shader: {e}")).ok());
    let gpu = if rain.is_some() { gpu } else { None };

    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x9e37_79b9, |d| d.as_nanos() as u64)
        ^ ((std::process::id() as u64) << 32);
    let font = load_font(&cfg.font);
    let mut app = App {
        loop_handle: event_loop.handle(),
        conn: conn.clone(),
        compositor: CompositorState::bind(&globals, &qh).expect("wl_compositor"),
        output_state: OutputState::new(&globals, &qh),
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        shm: Shm::bind(&globals, &qh).expect("wl_shm"),
        scaling: Scaling::bind(&globals, &qh),
        qh: qh.clone(),
        session_lock_state: SessionLockState::new(&globals, &qh),
        session_lock: None,
        outputs: Vec::new(),
        keyboard: None,
        rain,
        gpu,
        font,
        source: None,
        cfg,
        // Start a while into the rain, somewhere new each time, so the glass
        // is already wet when the lock goes up.
        rain_t: 30.0 + (seed % 3600) as f64,
        speed: 1.0,
        last_tick: Instant::now(),
        impacts: [Impact::NONE; IMPACTS],
        next_impact: 0,
        rng: seed | 1,
        wrong_at: None,
        unlock_at: None,
        dirty: true,
        caps_lock: false,
        status_msg: None,
        machine: Machine::new(),
        password: SecureBuf::new(),
        auth,
        mods: Modifiers::default(),
        locked: false,
        saw_first_key: false,
        exit: false,
        unlocked_ok: false,
        final_code: 1,
        auth_dead: false,
        input_idle_gen: 0,
        password_clear_gen: 0,
        auth_invalid_gen: 0,
    };

    let lock = match app.session_lock_state.lock(&qh) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("rainlock: ext-session-lock not supported: {e}");
            return 1;
        }
    };
    // A lock surface per existing output.
    let outputs: Vec<_> = app.output_state.outputs().collect();
    for output in outputs {
        app.add_output(&lock, output);
    }
    app.session_lock = Some(lock);

    // Decode the wallpaper off the event loop: the lock goes up at once over
    // the placeholder colour, and the picture replaces it when it is ready
    // (a few hundred ms for a large photo).
    if let Some(path) = app.cfg.image.clone() {
        let (tx, rx) = channel::channel::<Source>();
        std::thread::spawn(move || match rainglass::load_source(&path, SOURCE_MAX.0, SOURCE_MAX.1) {
            Ok(src) => {
                let _ = tx.send(src);
            }
            Err(e) => eprintln!("rainlock: wallpaper {e}; showing a plain colour"),
        });
        app.loop_handle
            .insert_source(rx, |event, _, app: &mut App| {
                if let channel::Event::Msg(src) = event {
                    app.source = Some(src);
                    app.dirty = true;
                }
            })
            .expect("insert wallpaper source");
    }

    // Poll the auth reply fd.
    let reply_fd = app.auth.reply_fd();
    let source = smithay_client_toolkit::reexports::calloop::generic::Generic::new(
        unsafe { std::os::unix::io::BorrowedFd::borrow_raw(reply_fd) }.try_clone_to_owned().unwrap(),
        smithay_client_toolkit::reexports::calloop::Interest::READ,
        smithay_client_toolkit::reexports::calloop::Mode::Level,
    );
    app.loop_handle
        .insert_source(source, |_, _, app: &mut App| {
            use smithay_client_toolkit::reexports::calloop::PostAction;
            // If the auth child died, its reply pipe is at permanent EOF —
            // a level-triggered source would then spin at 100% CPU. Remove it.
            let alive = app.on_auth_reply();
            Ok(if alive { PostAction::Continue } else { PostAction::Remove })
        })
        .expect("insert auth reply source");

    // SIGUSR1: an unauthenticated unlock is only wired up when explicitly
    // opted in. By default the signal is ignored so PAM is the sole unlock
    // path and no same-UID process can raise the lock.
    if app.cfg.allow_signal_unlock {
        install_sigusr1(&app.loop_handle);
    } else {
        unsafe {
            libc::signal(libc::SIGUSR1, libc::SIG_IGN);
        }
    }

    // Animation timer. A still glass only needs the clock and prompts kept
    // current, and keypresses shown promptly.
    let frame = if app.still_everywhere() {
        Duration::from_millis(250)
    } else {
        Duration::from_secs_f64(1.0 / app.cfg.fps)
    };
    app.loop_handle
        .insert_source(Timer::from_duration(frame), move |_, _, app: &mut App| {
            app.tick();
            TimeoutAction::ToDuration(app.next_tick_delay(frame))
        })
        .expect("insert frame timer");

    WaylandSource::new(conn, event_queue).insert(app.loop_handle.clone()).expect("insert wayland source");

    while !app.exit {
        if event_loop.dispatch(Duration::from_millis(100), &mut app).is_err() {
            break;
        }
    }

    if app.unlocked_ok {
        0
    } else {
        app.final_code
    }
}

// SIGUSR1 self-pipe -----------------------------------------------------------

static mut SIGUSR1_PIPE: [i32; 2] = [-1, -1];

extern "C" fn on_sigusr1(_sig: libc::c_int) {
    unsafe {
        let _ = libc::write(SIGUSR1_PIPE[1], b"1".as_ptr() as *const libc::c_void, 1);
    }
}

fn install_sigusr1(handle: &LoopHandle<'static, App>) {
    let read_fd = unsafe {
        if libc::pipe2(std::ptr::addr_of_mut!(SIGUSR1_PIPE) as *mut i32, libc::O_CLOEXEC | libc::O_NONBLOCK) != 0 {
            eprintln!("rainlock: warning: SIGUSR1 pipe failed; graceful unlock disabled");
            return;
        }
        libc::signal(libc::SIGUSR1, on_sigusr1 as *const () as libc::sighandler_t);
        SIGUSR1_PIPE[0]
    };
    let owned = unsafe { std::os::unix::io::BorrowedFd::borrow_raw(read_fd) }
        .try_clone_to_owned()
        .expect("dup sigusr1 read fd");
    let source = smithay_client_toolkit::reexports::calloop::generic::Generic::new(
        owned,
        smithay_client_toolkit::reexports::calloop::Interest::READ,
        smithay_client_toolkit::reexports::calloop::Mode::Level,
    );
    handle
        .insert_source(source, move |_, _, app: &mut App| {
            // Drain the self-pipe so the level-triggered source quiesces.
            let mut buf = [0u8; 8];
            while unsafe { libc::read(read_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) } > 0 {}
            app.begin_unlock(0); // scripted graceful-unlock escape hatch
            Ok(smithay_client_toolkit::reexports::calloop::PostAction::Continue)
        })
        .expect("insert sigusr1 source");
}

/// Local time "21:47" (localtime_r: no date/time dependency).
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
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
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

/// Lay the text out for one output: clock about a third of the way down,
/// date under it, then the status line. Sizes scale with the output.
fn band_for(font: &Font, c: &Content, cfg: &Cfg, bw: u32, bh: u32, logical_h: u32) -> Option<Band> {
    let s = bh as f32 / logical_h.max(1) as f32;
    let clock = cfg.clock_px * s;
    let lines = [
        Line { text: c.clock.clone(), px: clock, rgb: cfg.text, alpha: 0.95 },
        Line { text: c.date.clone(), px: clock * 0.25, rgb: cfg.text, alpha: 0.8 },
        Line { text: c.status.clone(), px: clock * 0.19, rgb: c.status_rgb, alpha: 0.95 },
    ];
    text::band(font, &lines, bw as usize, (bh as f32 * 0.3) as i32, (clock * 0.12) as usize)
}

impl App {
    fn add_output(&mut self, lock: &SessionLock, output: wl_output::WlOutput) {
        let int_scale = self.output_state.info(&output).map(|i| i.scale_factor).unwrap_or(1);
        let surface = self.compositor.create_surface(&self.qh);
        let scale = self.scaling.attach(&surface, &self.qh);
        let ls = lock.create_lock_surface(surface, &output, &self.qh);
        self.outputs.push(LockOutput::new(output, ls, scale, int_scale));
    }

    fn output_index(&self, surface: &wl_surface::WlSurface) -> Option<usize> {
        self.outputs.iter().position(|o| o.surface.wl_surface() == surface)
    }

    fn reset_output(&mut self, idx: usize) {
        self.outputs[idx].reset(self.gpu.as_ref());
    }

    /// Whether `idx` is drawn once per change rather than animated.
    fn still(&self, idx: usize) -> bool {
        self.cfg.still || self.gpu.is_none() || self.outputs[idx].cpu
    }

    fn still_everywhere(&self) -> bool {
        self.cfg.still || self.gpu.is_none()
    }

    fn rand(&mut self) -> f32 {
        // xorshift64*: plenty for drop positions.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    // ----- what the glass shows ---------------------------------------------

    fn splash(&mut self) {
        let (x, y) = (0.08 + 0.84 * self.rand(), 0.08 + 0.84 * self.rand());
        self.impacts[self.next_impact] = Impact { x, y, landed: self.rain_t as f32, wiped: -1.0 };
        self.next_impact = (self.next_impact + 1) % IMPACTS;
        self.dirty = true;
    }

    /// Backspace: the newest drop still on the glass runs off.
    fn wipe_newest(&mut self) {
        let newest = (0..IMPACTS)
            .filter(|&i| self.impacts[i].landed >= 0.0 && self.impacts[i].wiped < 0.0)
            .max_by(|&a, &b| self.impacts[a].landed.total_cmp(&self.impacts[b].landed));
        if let Some(i) = newest {
            self.impacts[i].wiped = self.rain_t as f32;
        }
        self.dirty = true;
    }

    fn wipe_all(&mut self) {
        let now = self.rain_t as f32;
        for i in self.impacts.iter_mut().filter(|i| i.landed >= 0.0 && i.wiped < 0.0) {
            i.wiped = now;
        }
        self.dirty = true;
    }

    fn wrong(&mut self) {
        self.wrong_at = Some(Instant::now());
        self.wipe_all();
    }

    fn content(&self) -> Content {
        let (status, status_rgb) = if self.unlock_at.is_some() {
            (String::new(), self.cfg.text)
        } else if self.auth_dead {
            ("authentication is broken: use the recovery key".into(), self.cfg.urgent)
        } else if self.machine.auth == AuthState::Validating {
            ("checking…".into(), self.cfg.text)
        } else if let Some(msg) = &self.status_msg {
            (msg.clone(), self.cfg.urgent)
        } else if self.machine.auth == AuthState::Invalid {
            ("wrong password".into(), self.cfg.urgent)
        } else if self.caps_lock {
            ("caps lock is on".into(), self.cfg.urgent)
        } else {
            (String::new(), self.cfg.text)
        };
        Content { clock: clock_hhmm(), date: clock_date(), status, status_rgb }
    }

    fn glass(&self) -> Frame {
        let mut f = Frame {
            time: self.rain_t as f32,
            density: self.cfg.density,
            dim: self.cfg.dim,
            impacts: self.impacts,
            ..Frame::default()
        };
        if let Some(at) = self.wrong_at {
            let p = at.elapsed().as_secs_f32() / WRONG_FLASH.as_secs_f32();
            if p < 1.0 {
                let u = self.cfg.urgent;
                f.tint = [u[0] as f32 / 255.0, u[1] as f32 / 255.0, u[2] as f32 / 255.0, 0.3 * (1.0 - p) * (1.0 - p)];
            }
        }
        if let Some(at) = self.unlock_at {
            let p = (at.elapsed().as_secs_f32() / UNLOCK_WIPE.as_secs_f32()).min(1.0);
            f.fog = 1.0 - p;
            f.dim += (1.0 - f.dim) * p;
        }
        f
    }

    // ----- frames -------------------------------------------------------------

    fn tick(&mut self) {
        if self.exit {
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.25);
        self.last_tick = now;
        // The rain all but stops while PAM decides, and eases back after.
        let target = if self.machine.auth == AuthState::Validating { 0.15 } else { 1.0 };
        self.speed += (target - self.speed) * (dt * 4.0).min(1.0);
        if !self.cfg.still {
            self.rain_t += dt * self.speed;
        }

        let content = self.content();
        for i in 0..self.outputs.len() {
            if !self.outputs[i].configured {
                continue;
            }
            // A still output repaints only when something it shows changed.
            if self.still(i) && !self.dirty && !self.text_changed(i, &content) {
                continue;
            }
            if self.outputs[i].frame_pending {
                // Frame-callback watchdog: see FRAME_WATCHDOG.
                let wait = self.outputs[i].watchdog_wait;
                let stuck = self.outputs[i].frame_pending_since.is_some_and(|since| since.elapsed() >= wait);
                if !stuck {
                    continue;
                }
                if !self.outputs[i].watchdog_tripped {
                    self.outputs[i].watchdog_tripped = true;
                    eprintln!("rainlock: frame callback lost on output {i} (DPMS/hotplug?) — repainting");
                }
                self.outputs[i].watchdog_wait = watchdog_backoff(wait);
                self.outputs[i].frame_pending = false;
            }
            self.paint(i, &content);
        }
        self.dirty = false;
    }

    fn text_changed(&self, idx: usize, content: &Content) -> bool {
        let o = &self.outputs[idx];
        let (bw, bh) = o.buffer_size();
        o.text_key != format!("{}@{bw}x{bh}", content.key())
    }

    fn paint(&mut self, idx: usize, content: &Content) {
        let (w, h) = (self.outputs[idx].width, self.outputs[idx].height);
        if w == 0 || h == 0 {
            return;
        }
        let t0 = self.cfg.debug_timing.then(Instant::now);
        let drawn = !self.outputs[idx].cpu && self.gpu.is_some() && self.paint_gpu(idx, content);
        if !drawn {
            self.paint_cpu(idx, content);
        }
        if let Some(t0) = t0 {
            eprintln!("rainlock: frame {:.2}ms", t0.elapsed().as_secs_f64() * 1000.0);
        }
    }

    /// One frame on the GPU. False if this output can't be drawn that way
    /// (it is then marked for the CPU path).
    fn paint_gpu(&mut self, idx: usize, content: &Content) -> bool {
        let frame = self.glass();
        let (Some(gpu), Some(rain)) = (self.gpu.as_ref(), self.rain.as_ref()) else { return false };
        let o = &mut self.outputs[idx];
        let (bw, bh) = o.buffer_size();

        if o.target.is_none() {
            o.prepare_surface();
            match gpu.target(o.surface.wl_surface().id().as_ptr().cast(), bw as i32, bh as i32) {
                Ok(t) => o.target = Some(t),
                Err(e) => {
                    eprintln!("rainlock: output {idx}: {e}; drawing it on the CPU");
                    o.cpu = true;
                    return false;
                }
            }
        }
        if let Err(e) = gpu.bind(o.target.as_ref().unwrap()) {
            eprintln!("rainlock: {e}");
            o.cpu = true;
            o.target = None;
            return false;
        }

        // The wallpaper, fitted to this output (the placeholder until it has
        // decoded).
        let want_wallpaper = self.source.is_some();
        if o.plate.is_none() || (want_wallpaper && !o.has_wallpaper) {
            if let Some(p) = o.plate.take() {
                p.free(gpu);
            }
            let bg = match &self.source {
                Some(src) => Background::from_source(src, bw, bh),
                None => Background::solid(PLACEHOLDER, bw, bh),
            };
            match Plate::new(gpu, &bg) {
                Ok(p) => {
                    o.plate = Some(p);
                    o.has_wallpaper = want_wallpaper;
                    o.text_key.clear();
                }
                Err(e) => {
                    eprintln!("rainlock: output {idx}: {e}; drawing it on the CPU");
                    o.cpu = true;
                    o.target = None;
                    return false;
                }
            }
        }
        let key = format!("{}@{bw}x{bh}", content.key());
        if o.text_key != key {
            let band = self.font.as_ref().and_then(|f| band_for(f, content, &self.cfg, bw, bh, o.height));
            if let Err(e) = o.plate.as_mut().unwrap().set_text(gpu, band.as_ref()) {
                eprintln!("rainlock: text: {e}");
            }
            o.text_key = key;
        }

        o.prepare_surface();
        rain.draw(gpu, o.plate.as_ref().unwrap(), &frame);
        let surface = o.surface.wl_surface();
        surface.frame(&self.qh, surface.clone());
        if let Err(e) = gpu.present(o.target.as_ref().unwrap()) {
            eprintln!("rainlock: {e}");
            return true; // the next tick tries again
        }
        o.frame_pending = true;
        o.frame_pending_since = Some(Instant::now());
        true
    }

    /// One still frame on the CPU: the frosted wallpaper and the text.
    fn paint_cpu(&mut self, idx: usize, content: &Content) {
        let o = &mut self.outputs[idx];
        let (bw, bh) = o.buffer_size();
        let want_wallpaper = self.source.is_some();
        if o.cpu_bg.is_none() || (want_wallpaper && !o.has_wallpaper) {
            let mut bg = match &self.source {
                Some(src) => Background::from_source(src, bw, bh),
                None => Background::solid(PLACEHOLDER, bw, bh),
            };
            bg.sharp = Vec::new(); // only the frosted copy is drawn
            o.cpu_bg = Some(bg);
            o.has_wallpaper = want_wallpaper;
        }
        let stride = bw as i32 * 4;
        if o.pool.is_none() {
            match SlotPool::new((stride * bh as i32) as usize, &self.shm) {
                Ok(p) => o.pool = Some(p),
                Err(e) => {
                    eprintln!("rainlock: shm pool failed: {e}");
                    return;
                }
            }
        }
        let band = self.font.as_ref().and_then(|f| band_for(f, content, &self.cfg, bw, bh, o.height));
        let pool = o.pool.as_mut().unwrap();
        match pool.create_buffer(bw as i32, bh as i32, stride, wl_shm::Format::Xrgb8888) {
            Ok((buffer, canvas)) => {
                rainglass::still::paint(canvas, bw as usize, bh as usize, o.cpu_bg.as_ref().unwrap(), band.as_ref(), self.cfg.dim);
                o.prepare_surface();
                let surface = o.surface.wl_surface();
                surface.frame(&self.qh, surface.clone());
                buffer.attach_to(surface).expect("attach");
                // Without damage the compositor never repaints the output.
                surface.damage_buffer(0, 0, bw as i32, bh as i32);
                surface.commit();
                o.text_key = format!("{}@{bw}x{bh}", content.key());
                o.frame_pending = true;
                o.frame_pending_since = Some(Instant::now());
            }
            Err(e) => eprintln!("rainlock: buffer alloc failed: {e}"),
        }
    }

    /// How long until the next animation tick. While EVERY configured output
    /// sits on an unanswered frame callback there is no screen to animate
    /// for, so the timer stretches to the soonest watchdog: a locked laptop
    /// with its monitors off wakes every few seconds, not 30 times a second.
    fn next_tick_delay(&self, frame: Duration) -> Duration {
        let mut soonest: Option<Duration> = None;
        for o in self.outputs.iter().filter(|o| o.configured) {
            if !(o.frame_pending && o.watchdog_tripped) {
                return frame;
            }
            let waited = o.frame_pending_since.map_or(Duration::ZERO, |s| s.elapsed());
            let due = o.watchdog_wait.saturating_sub(waited);
            soonest = Some(soonest.map_or(due, |s: Duration| s.min(due)));
        }
        soonest.unwrap_or(frame).max(frame)
    }

    // ----- keyboard -----------------------------------------------------------

    fn on_key(&mut self, event: KeyEvent) {
        // Somebody is at the keyboard, so the screen is on (niri wakes the
        // monitors on input): drop any watchdog backoff built up in the dark.
        for o in &mut self.outputs {
            o.reset_watchdog();
        }
        if self.exit || self.unlock_at.is_some() {
            return;
        }
        let ctrl = self.mods.ctrl;
        match key_action(event.keysym, event.utf8.as_deref(), ctrl) {
            Action::Append(mut s) => {
                self.password.push_str(&s);
                // Wipe this transient heap copy once it's in the mlock'd buffer
                // (the KeyEvent's own String is dropped by sctk right after).
                s.zeroize();
                self.machine.on_letter();
                self.splash();
                self.arm_password_clear();
                self.arm_input_idle();
            }
            Action::PopChar => {
                if self.password.pop_char() {
                    let now_empty = self.password.is_empty();
                    self.machine.on_backspace(now_empty);
                    if now_empty {
                        self.wipe_all();
                    } else {
                        self.wipe_newest();
                        self.arm_password_clear();
                    }
                    self.arm_input_idle();
                }
            }
            Action::ClearAll => {
                self.password.clear();
                self.machine.on_clear();
                self.wipe_all();
                self.arm_input_idle();
            }
            Action::Submit => {
                if self.password.is_empty() {
                    return; // ignore-empty-password
                }
                if self.auth_dead {
                    // No authenticator left; can't succeed. Show the failure
                    // without touching the dead pipe.
                    self.password.clear();
                    self.machine.on_submit();
                    self.machine.on_reply(false);
                    self.wrong();
                    self.arm_auth_invalid();
                    return;
                }
                let (go, _) = self.machine.on_submit();
                if go {
                    self.status_msg = None; // stale PAM text
                    self.dirty = true;
                    let ok = self.auth.submit(&self.password);
                    self.password.clear(); // clear whether or not the write took
                    if !ok {
                        // Auth child gone: don't leave the state machine stuck
                        // in Validating (which would block every retry).
                        eprintln!("rainlock: auth channel broke");
                        self.machine.on_reply(false);
                        self.wrong();
                        self.arm_auth_invalid();
                    }
                }
            }
            Action::Neutral => {
                self.machine.on_neutral();
                self.arm_password_clear();
                self.arm_input_idle();
            }
            Action::Ignore => {}
        }
    }

    /// Handle an auth verdict. Returns false if the auth child is gone (so the
    /// caller removes the reply source to avoid a busy-loop on EOF).
    fn on_auth_reply(&mut self) -> bool {
        self.dirty = true;
        match self.auth.read_reply() {
            Some((true, _)) => {
                self.machine.on_reply(true);
                self.start_unlock();
                true
            }
            Some((false, msg)) => {
                self.machine.on_reply(false);
                self.wrong();
                // PAM's own words (faillock lockout etc.) beat a bare flash;
                // they stay up until the next submit.
                self.status_msg = msg;
                self.arm_auth_invalid();
                true
            }
            None => {
                // Auth child died. The session stays locked (fail-secure);
                // recover with the recovery bind or a TTY.
                self.machine.auth = AuthState::Idle;
                self.auth_dead = true;
                eprintln!("rainlock: auth child died — session stays locked; use the recovery bind");
                false
            }
        }
    }

    // ----- timers (calloop one-shots keyed by a generation counter) ----------

    fn arm_input_idle(&mut self) {
        self.input_idle_gen += 1;
        let gen = self.input_idle_gen;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_secs_f64(crate::input::INPUT_IDLE_SECS)),
            move |_, _, app: &mut App| {
                if app.input_idle_gen == gen {
                    app.machine.on_input_idle();
                }
                TimeoutAction::Drop
            },
        );
    }

    fn arm_password_clear(&mut self) {
        self.password_clear_gen += 1;
        let gen = self.password_clear_gen;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_secs_f64(crate::input::PASSWORD_CLEAR_SECS)),
            move |_, _, app: &mut App| {
                if app.password_clear_gen == gen && !app.password.is_empty() {
                    app.password.clear();
                    app.machine.on_clear();
                    app.wipe_all();
                }
                TimeoutAction::Drop
            },
        );
    }

    fn arm_auth_invalid(&mut self) {
        self.auth_invalid_gen += 1;
        let gen = self.auth_invalid_gen;
        let _ = self.loop_handle.insert_source(
            Timer::from_duration(Duration::from_secs_f64(crate::input::AUTH_INVALID_SECS)),
            move |_, _, app: &mut App| {
                if app.auth_invalid_gen == gen {
                    app.machine.on_auth_idle();
                    app.dirty = true;
                }
                TimeoutAction::Drop
            },
        );
    }

    // ----- unlock -------------------------------------------------------------

    /// PAM said yes: wipe the fog away, then unlock. A still glass has nothing
    /// to animate, so it unlocks at once.
    fn start_unlock(&mut self) {
        self.password.clear();
        if self.still_everywhere() {
            self.begin_unlock(0);
            return;
        }
        self.unlock_at = Some(Instant::now());
        let _ = self.loop_handle.insert_source(Timer::from_duration(UNLOCK_WIPE), |_, _, app: &mut App| {
            app.begin_unlock(0);
            TimeoutAction::Drop
        });
    }

    fn begin_unlock(&mut self, code: i32) {
        if self.exit {
            return;
        }
        self.password.clear();
        if let Some(lock) = self.session_lock.take() {
            lock.unlock();
            // Roundtrip so the compositor processes unlock before we exit;
            // otherwise the connection could tear down first and the session
            // would stay locked.
            let _ = self.conn.roundtrip();
        }
        self.unlocked_ok = code == 0;
        self.final_code = code;
        self.exit = true;
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        self.locked = true;
        eprintln!("rainlock: session locked (compositor confirmed)");
        // Ordering matters: signal readiness, THEN daemonize — both only after
        // the compositor confirms the session is locked, so
        // `rainlock -f && systemctl suspend` cannot race.
        if let Some(fd) = self.cfg.ready_fd.take() {
            unsafe {
                let _ = libc::write(fd, b"\n".as_ptr() as *const libc::c_void, 1);
                libc::close(fd);
            }
        }
        if self.cfg.daemonize {
            crate::daemon::finish_daemonize();
        }
    }

    fn finished(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _lock: SessionLock) {
        eprintln!("rainlock: failed to lock session — is another locker already running?");
        self.session_lock = None;
        self.unlocked_ok = false;
        self.final_code = 2;
        self.exit = true;
        // If we were about to daemonize, the parent is still blocked on the
        // status pipe; tell it we failed so `&&` chains halt.
        if self.cfg.daemonize {
            crate::daemon::abort_daemonize();
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        let Some(idx) = self.output_index(surface.wl_surface()) else { return };
        let resized = self.outputs[idx].width != w || self.outputs[idx].height != h;
        eprintln!("rainlock: lock surface configured {w}x{h}{}", if resized { " (resized)" } else { "" });
        if resized {
            self.reset_output(idx);
        }
        let o = &mut self.outputs[idx];
        o.width = w;
        o.height = h;
        o.configured = true;
        o.frame_pending = false;
        o.frame_pending_since = None;
        o.watchdog_tripped = false;
        // Paint immediately (ack is handled internally by sctk).
        let content = self.content();
        self.paint(idx, &content);
    }
}

impl ScaleHandler for App {
    fn preferred_scale(&mut self, surface: &wl_surface::WlSurface, scale120: u32) {
        let Some(idx) = self.output_index(surface) else { return };
        if self.outputs[idx].scale120 != scale120 {
            self.reset_output(idx);
            self.outputs[idx].scale120 = scale120;
            self.dirty = true;
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        // Integer scale only matters without fractional scaling.
        let Some(idx) = self.output_index(surface) else { return };
        let s120 = factor.max(1) as u32 * 120;
        if !self.outputs[idx].scale.fractional() && self.outputs[idx].scale120 != s120 {
            self.reset_output(idx);
            self.outputs[idx].scale120 = s120;
            self.dirty = true;
        }
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(idx) = self.output_index(surface) {
            let o = &mut self.outputs[idx];
            if o.watchdog_tripped {
                o.watchdog_tripped = false;
                eprintln!("rainlock: frame callbacks resumed");
            }
            o.reset_watchdog();
            o.frame_pending = false;
            o.frame_pending_since = None;
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        // A monitor plugged in while locked MUST get a lock surface, or it
        // would show unlocked content. (sctk's example omits this.)
        let Some(lock) = self.session_lock.clone() else { return };
        if !self.outputs.iter().any(|o| o.output == output) {
            let name = self.output_state.info(&output).and_then(|i| i.name).unwrap_or_default();
            eprintln!("rainlock: output {name:?} appeared — creating lock surface");
            self.add_output(&lock, output);
        }
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let Some(idx) = self.outputs.iter().position(|o| o.output == output) else { return };
        self.reset_output(idx);
        self.outputs.remove(idx);
        eprintln!("rainlock: output gone — dropped its lock surface ({} left)", self.outputs.len());
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _conn: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard_with_repeat(
                qh,
                &seat,
                None,
                self.loop_handle.clone(),
                Box::new(|app: &mut App, _kbd, event| app.on_key(event)),
            ) {
                Ok(kbd) => {
                    eprintln!("rainlock: keyboard acquired");
                    self.keyboard = Some(kbd);
                }
                Err(e) => eprintln!("rainlock: no keyboard: {e}"),
            }
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Keyboard {
            if let Some(kbd) = self.keyboard.take() {
                eprintln!("rainlock: keyboard capability removed");
                kbd.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {
        // Input-path diagnostic: if this never fires, the compositor never gave
        // the lock surface keyboard focus — the bug is upstream of us.
        eprintln!("rainlock: keyboard focus entered lock surface");
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {
        eprintln!("rainlock: keyboard focus left lock surface");
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        if !self.saw_first_key {
            self.saw_first_key = true;
            // Deliberately content-free: proves delivery without logging keys.
            eprintln!("rainlock: first key event received");
        }
        self.on_key(event);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {
        // Compositor-driven (wl_keyboard v10) repeats — no-op. Repeat is driven
        // by the get_keyboard_with_repeat calloop callback, so handling this
        // too would double-type on v10 compositors.
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, modifiers: Modifiers, _: RawModifiers, _: u32) {
        self.mods = modifiers;
        if self.caps_lock != modifiers.caps_lock {
            self.caps_lock = modifiers.caps_lock;
            self.dirty = true;
        }
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
smithay_client_toolkit::delegate_session_lock!(App);
smithay_client_toolkit::delegate_registry!(App);
rainglass::delegate_scaling!(App);

#[cfg(test)]
mod tests {
    use super::*;

    /// The watchdog must keep answering promptly at first (a genuinely lost
    /// callback with someone watching) and then get out of the way (a monitor
    /// that has been powered off for the night).
    #[test]
    fn watchdog_backs_off_by_doubling_and_stops_at_the_cap() {
        let mut w = FRAME_WATCHDOG;
        assert_eq!(w, Duration::from_millis(250));
        w = watchdog_backoff(w);
        assert_eq!(w, Duration::from_millis(500));
        w = watchdog_backoff(w);
        assert_eq!(w, Duration::from_secs(1));
        for _ in 0..10 {
            w = watchdog_backoff(w);
        }
        assert_eq!(w, FRAME_WATCHDOG_MAX);
        assert_eq!(watchdog_backoff(w), FRAME_WATCHDOG_MAX);
    }

    #[test]
    fn content_key_changes_with_what_is_shown() {
        let a = Content { clock: "21:47".into(), date: "SUN 18 OCT".into(), status: String::new(), status_rgb: [1, 2, 3] };
        let b = Content { status: "wrong password".into(), ..Content { clock: a.clock.clone(), date: a.date.clone(), status: String::new(), status_rgb: a.status_rgb } };
        assert_ne!(a.key(), b.key());
    }
}
