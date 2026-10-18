// SPDX-License-Identifier: GPL-3.0-or-later
// Options: defaults, then ~/.config/rainlock/theme (written by the dotfiles'
// `theme` command), then ~/.config/rainlock/config (key=value), then flags.

pub struct Cfg {
    pub daemonize: bool,
    pub ready_fd: Option<i32>,
    /// Opt-in: honor SIGUSR1 as a scripted unlock. Off by default so PAM is
    /// the only unlock path (a same-UID process must not be able to unlock).
    pub allow_signal_unlock: bool,
    /// The picture behind the glass. None: the session's wallpaper, recorded
    /// in $XDG_STATE_HOME/niri-wallpaper, else a plain colour.
    pub image: Option<String>,
    pub font: String,
    pub fps: f64,
    /// 0..1, the share of the glass holding drops.
    pub density: f32,
    /// Overall brightness behind the text.
    pub dim: f32,
    /// Clock size in logical pixels (scaled to the output).
    pub clock_px: f32,
    pub text: [u8; 3],
    pub urgent: [u8; 3],
    /// Draw once and never animate (slow GPUs, or for less distraction).
    pub still: bool,
    pub debug_timing: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            daemonize: false,
            ready_fd: None,
            allow_signal_unlock: false,
            image: None,
            font: "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Light.ttf".into(),
            fps: 30.0,
            density: 0.7,
            dim: 0.85,
            clock_px: 120.0,
            text: [236, 239, 244],  // #eceff4
            urgent: [224, 108, 117], // #e06c75
            still: false,
            debug_timing: false,
        }
    }
}

const USAGE: &str = "rainlock — ext-session-lock screen locker for niri\n\
    Your wallpaper through a rainy window; type your password to unlock.\n\n\
    -f, --daemonize    fork; parent exits 0 once the session is locked\n\
    --ready-fd N       write one byte to fd N once the session is locked\n\
    --allow-signal-unlock  honor SIGUSR1 as a scripted unlock (OFF by default;\n\
                       leaving it off keeps PAM the only unlock path)\n\
    --image PATH       picture behind the glass (default: the session's\n\
                       wallpaper, from $XDG_STATE_HOME/niri-wallpaper)\n\
    --font PATH        TTF for the clock and prompts\n\
    --fps N            frames per second             (default 30)\n\
    --density F        how much rain, 0..1           (default 0.7)\n\
    --dim F            brightness behind the text    (default 0.85)\n\
    --clock-px PX      clock size, logical px        (default 120)\n\
    --text HEX         text colour                   (default #eceff4)\n\
    --urgent HEX       error colour                  (default #e06c75)\n\
    --still            draw once, no animation\n\
    --debug-timing     print per-frame render times\n\n\
    The same keys (without the dashes) work in ~/.config/rainlock/config\n\
    as key=value lines; flags win. ~/.config/rainlock/theme, which the\n\
    `theme` command writes, is read first, so the config file wins over it.\n";

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?])
}

/// `~/x` -> `$HOME/x`.
fn expand(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()),
        None => path.to_string(),
    }
}

/// Strip a trailing `# comment`, but only a `#` that starts the line or
/// follows whitespace, so hex colours (`text=#eceff4`) survive.
fn strip_comment(raw: &str) -> &str {
    let bytes = raw.as_bytes();
    for (i, _) in raw.match_indices('#') {
        if i == 0 || bytes[i - 1].is_ascii_whitespace() {
            return &raw[..i];
        }
    }
    raw
}

/// Apply one option. Unknown keys and bad values are reported, not fatal:
/// a typo in a config file must never stop the screen from locking.
fn set(cfg: &mut Cfg, key: &str, val: &str) -> bool {
    let num = |v: &str| v.trim().parse::<f64>().ok();
    match key {
        "image" => cfg.image = Some(expand(val.trim())),
        "font" => cfg.font = expand(val.trim()),
        "fps" => cfg.fps = num(val).unwrap_or(cfg.fps),
        "density" => cfg.density = num(val).map_or(cfg.density, |v| v as f32),
        "dim" => cfg.dim = num(val).map_or(cfg.dim, |v| v as f32),
        "clock-px" | "clock_px" => cfg.clock_px = num(val).map_or(cfg.clock_px, |v| v as f32),
        "text" => cfg.text = parse_hex(val).unwrap_or(cfg.text),
        "urgent" => cfg.urgent = parse_hex(val).unwrap_or(cfg.urgent),
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
            eprintln!("rainlock: {path}: unknown key {:?} (ignored)", k.trim());
        }
    }
}

/// The wallpaper the session is showing, as recorded by its wallpaper setter.
fn session_wallpaper() -> Option<String> {
    let state = std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.local/state", std::env::var("HOME").unwrap_or_default()));
    let path = std::fs::read_to_string(format!("{state}/niri-wallpaper")).ok()?;
    let path = path.trim();
    std::path::Path::new(path).is_file().then(|| path.to_string())
}

pub fn parse() -> Cfg {
    let mut cfg = Cfg::default();
    let home = std::env::var("HOME").unwrap_or_default();
    load_file(&mut cfg, &format!("{home}/.config/rainlock/theme"));
    load_file(&mut cfg, &format!("{home}/.config/rainlock/config"));
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "-f" | "--daemonize" => cfg.daemonize = true,
            "--allow-signal-unlock" => cfg.allow_signal_unlock = true,
            "--still" => cfg.still = true,
            "--debug-timing" => cfg.debug_timing = true,
            "--help" | "-h" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--ready-fd" => {
                cfg.ready_fd = args.next().and_then(|v| v.parse().ok());
                if cfg.ready_fd.is_none() {
                    eprintln!("--ready-fd needs an fd number");
                    std::process::exit(2);
                }
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
    if cfg.image.is_none() {
        cfg.image = session_wallpaper();
    }
    cfg.fps = cfg.fps.clamp(1.0, 240.0);
    cfg.density = cfg.density.clamp(0.0, 1.0);
    cfg.dim = cfg.dim.clamp(0.1, 1.0);
    cfg.clock_px = cfg.clock_px.clamp(16.0, 400.0);
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_values_survive_comment_stripping() {
        assert_eq!(strip_comment("text=#eceff4"), "text=#eceff4");
        assert_eq!(strip_comment("density=0.5   # lighter rain"), "density=0.5   ");
        assert_eq!(strip_comment("# whole line"), "");
    }

    #[test]
    fn config_file_keys_load() {
        let path = std::env::temp_dir().join("rainlock-cli-test-config");
        std::fs::write(&path, "density=0.4  # light rain\ntext=#112233\nbogus=1\n").unwrap();
        let mut cfg = Cfg::default();
        load_file(&mut cfg, path.to_str().unwrap());
        let _ = std::fs::remove_file(&path);
        assert_eq!(cfg.density, 0.4);
        assert_eq!(cfg.text, [0x11, 0x22, 0x33]);
        assert_eq!(cfg.urgent, Cfg::default().urgent);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#e06c75"), Some([224, 108, 117]));
        assert_eq!(parse_hex("e06c75"), Some([224, 108, 117]));
        assert_eq!(parse_hex("#e06c7"), None);
        assert_eq!(parse_hex("#zz6c75"), None);
    }
}
