// SPDX-License-Identifier: GPL-3.0-or-later
// Options: defaults, then /etc/raingreet/config (key=value), then flags. The
// greeter runs as the `greeter` user, so everything it reads lives outside
// any home directory.

pub struct Cfg {
    /// The picture behind the glass. The installer copies a wallpaper here,
    /// since the greeter user can't read anyone's home.
    pub image: String,
    pub font: String,
    pub fps: f64,
    pub density: f32,
    pub dim: f32,
    /// Clock size in logical pixels.
    pub clock_px: f32,
    pub text: [u8; 3],
    pub urgent: [u8; 3],
    pub still: bool,
    pub debug_timing: bool,
    /// Where sessions are listed, earlier dirs first.
    pub sessions_dirs: Vec<String>,
    /// Session command when no session files are found.
    pub cmd: Vec<String>,
    /// The last user and session, prefilled at the next boot. Empty: don't
    /// remember.
    pub remember: String,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            image: "/usr/share/raingreet/background.jpg".into(),
            font: "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Light.ttf".into(),
            fps: 30.0,
            density: 0.7,
            dim: 0.85,
            clock_px: 120.0,
            text: [236, 239, 244],
            urgent: [224, 108, 117],
            still: false,
            debug_timing: false,
            sessions_dirs: vec!["/usr/local/share/wayland-sessions".into(), "/usr/share/wayland-sessions".into()],
            cmd: vec!["niri-session".into()],
            remember: "/var/cache/raingreet/last".into(),
        }
    }
}

const USAGE: &str = "raingreet — greetd greeter: your wallpaper through a rainy window\n\
    Run under cage by greetd: command = \"cage -s -- raingreet\"\n\n\
    --image PATH       picture behind the glass (default /usr/share/raingreet/background.jpg)\n\
    --font PATH        TTF for the clock and prompts\n\
    --fps N            frames per second             (default 30)\n\
    --density F        how much rain, 0..1           (default 0.7)\n\
    --dim F            brightness behind the text    (default 0.85)\n\
    --clock-px PX      clock size, logical px        (default 120)\n\
    --text HEX         text colour                   (default #eceff4)\n\
    --urgent HEX       error colour                  (default #e06c75)\n\
    --sessions DIRS    colon-separated session dirs  (default /usr/local/share/\n\
                       wayland-sessions:/usr/share/wayland-sessions)\n\
    --cmd \"CMD ...\"    session command if none are found (default niri-session)\n\
    --remember PATH    last user and session         (default /var/cache/raingreet/last;\n\
                       \"\" to not remember)\n\
    --still            draw once, no animation\n\
    --debug-timing     print per-frame render times\n\n\
    The same keys (without the dashes) work in /etc/raingreet/config as\n\
    key=value lines; flags win.\n\
    Keys: F2 picks the session, Esc at an empty password goes back to the\n\
    username, Ctrl+Alt+Delete reboots.\n";

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

fn strip_comment(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    for (i, _) in raw.match_indices('#') {
        if i == 0 || bytes[i - 1].is_ascii_whitespace() {
            return &raw[..i];
        }
    }
    raw
}

/// Apply one option. A bad value keeps the default: a typo in a config file
/// must never keep the login screen from coming up.
fn set(cfg: &mut Cfg, key: &str, val: &str) -> bool {
    let val = val.trim();
    let num = |v: &str| v.parse::<f64>().ok();
    match key {
        "image" => cfg.image = val.into(),
        "font" => cfg.font = val.into(),
        "fps" => cfg.fps = num(val).unwrap_or(cfg.fps),
        "density" => cfg.density = num(val).map_or(cfg.density, |v| v as f32),
        "dim" => cfg.dim = num(val).map_or(cfg.dim, |v| v as f32),
        "clock-px" | "clock_px" => cfg.clock_px = num(val).map_or(cfg.clock_px, |v| v as f32),
        "text" => cfg.text = parse_hex(val).unwrap_or(cfg.text),
        "urgent" => cfg.urgent = parse_hex(val).unwrap_or(cfg.urgent),
        "sessions" => cfg.sessions_dirs = val.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect(),
        "cmd" => {
            let cmd: Vec<String> = val.split_whitespace().map(str::to_string).collect();
            if !cmd.is_empty() {
                cfg.cmd = cmd;
            }
        }
        "remember" => cfg.remember = val.into(),
        _ => return false,
    }
    true
}

fn load_file(cfg: &mut Cfg, path: &str) {
    let Ok(body) = std::fs::read_to_string(path) else { return };
    for raw in body.lines() {
        let line = strip_comment(raw).trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        if !set(cfg, k.trim(), v) {
            eprintln!("raingreet: {path}: unknown key {:?} (ignored)", k.trim());
        }
    }
}

pub fn parse() -> Cfg {
    let mut cfg = Cfg::default();
    load_file(&mut cfg, "/etc/raingreet/config");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--still" => cfg.still = true,
            "--debug-timing" => cfg.debug_timing = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            flag if flag.starts_with("--") => {
                let Some(v) = args.next() else {
                    eprintln!("missing value for {flag}");
                    std::process::exit(2);
                };
                if !set(&mut cfg, &flag[2..], &v) {
                    eprintln!("unknown flag {flag}\n\n{USAGE}");
                    std::process::exit(2);
                }
            }
            other => {
                eprintln!("unknown argument {other}\n\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    cfg.fps = cfg.fps.clamp(1.0, 240.0);
    cfg.density = cfg.density.clamp(0.0, 1.0);
    cfg.dim = cfg.dim.clamp(0.1, 1.0);
    cfg.clock_px = cfg.clock_px.clamp(16.0, 400.0);
    cfg
}

/// The remembered user and session, from `path`.
pub fn recall(path: &str) -> (Option<String>, Option<String>) {
    let (mut user, mut session) = (None, None);
    if path.is_empty() {
        return (user, session);
    }
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        match line.split_once('=') {
            Some(("user", v)) if !v.trim().is_empty() => user = Some(v.trim().to_string()),
            Some(("session", v)) if !v.trim().is_empty() => session = Some(v.trim().to_string()),
            _ => {}
        }
    }
    (user, session)
}

/// Remember `user` and `session` for the next boot. Best effort: a read-only
/// or missing cache dir only means retyping the username.
pub fn remember(path: &str, user: &str, session: &str) {
    if path.is_empty() {
        return;
    }
    let clean = |s: &str| s.replace(['\n', '\r'], " ");
    if let Err(e) = std::fs::write(path, format!("user={}\nsession={}\n", clean(user), clean(session))) {
        eprintln!("raingreet: can't remember the last login in {path}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_file_keys_load() {
        let path = std::env::temp_dir().join(format!("raingreet-cli-test-{}", std::process::id()));
        std::fs::write(&path, "density=0.4  # light\nsessions=/a:/b\nimage=/x.jpg\nnope=1\n").unwrap();
        let mut cfg = Cfg::default();
        load_file(&mut cfg, path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(cfg.density, 0.4);
        assert_eq!(cfg.sessions_dirs, vec!["/a", "/b"]);
        assert_eq!(cfg.image, "/x.jpg");
    }

    #[test]
    fn remember_round_trips_and_strips_newlines() {
        let path = std::env::temp_dir().join(format!("raingreet-last-test-{}", std::process::id()));
        let p = path.to_str().unwrap();
        remember(p, "whyte", "Life\nBranch");
        let (user, session) = recall(p);
        let _ = std::fs::remove_file(&path);
        assert_eq!(user.as_deref(), Some("whyte"));
        assert_eq!(session.as_deref(), Some("Life Branch"));
        assert_eq!(recall(""), (None, None));
    }
}
