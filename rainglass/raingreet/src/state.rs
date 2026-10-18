// SPDX-License-Identifier: GPL-3.0-or-later
// The greeter phase machine: username box -> cube grows -> password ->
// validating -> session start (or failure collapse back to the box).
// Pure logic: the app layer owns the clock, timers, IPC, and the buffers,
// and calls these transitions; this file decides what is allowed when.

pub const GROW_SECS: f64 = 0.8; // ease-out cubic
pub const COLLAPSE_SECS: f64 = 0.45; // ease-in (slow start, fast finish)
/// Failure hold before collapsing: longer than scene::WRONG_SECS (0.4) so
/// the rust flash and the reseed both read fully at cube scale 1.
pub const FAILED_HOLD_SECS: f64 = 0.9;
/// Idle in the password phase with an empty buffer -> collapse back to the
/// username box (the username must be re-entered every time by design).
pub const IDLE_COLLAPSE_SECS: f64 = 45.0;
pub const USERNAME_MAX: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    /// Username box + clock + session line; NO cube (all-ember map).
    EnterUser,
    /// Cube scaling up out of the box; password keys are already accepted.
    Growing { since: f64 },
    EnterPassword,
    /// Submission in flight; input ignored, verify wave plays.
    Validating,
    /// Wrong password: rust flash + reseed playing at full scale.
    Failed { since: f64 },
    /// Cube scaling back down; input ignored. Ends in EnterUser, cleared.
    Collapsing { since: f64 },
    /// start_session sent; waiting for greetd's Success, then we exit 0.
    Starting,
}

pub struct Greeter {
    pub phase: Phase,
    pub username: String,
}

fn ease_out_cubic(x: f64) -> f64 {
    let inv = 1.0 - x.clamp(0.0, 1.0);
    1.0 - inv * inv * inv
}

impl Greeter {
    pub fn new() -> Greeter {
        Greeter { phase: Phase::EnterUser, username: String::new() }
    }

    /// Cube silhouette scale 0..=1 for scene time `t` (drives map choice).
    pub fn cube_scale(&self, t: f64) -> f64 {
        match self.phase {
            Phase::EnterUser => 0.0,
            Phase::Growing { since } => ease_out_cubic((t - since) / GROW_SECS),
            Phase::EnterPassword | Phase::Validating | Phase::Failed { .. } | Phase::Starting => 1.0,
            Phase::Collapsing { since } => {
                // Ease-in: shrinks slowly, then collapses fast into the box.
                let x = ((t - since) / COLLAPSE_SECS).clamp(0.0, 1.0);
                1.0 - x * x * x
            }
        }
    }

    /// Username box border/text opacity: solid in EnterUser, fading over the
    /// first 40% of the grow, reappearing over the last 40% of the collapse.
    pub fn box_alpha(&self, t: f64) -> f64 {
        match self.phase {
            Phase::EnterUser => 1.0,
            Phase::Growing { since } => 1.0 - ((t - since) / (GROW_SECS * 0.4)).clamp(0.0, 1.0),
            Phase::Collapsing { since } => {
                let x = ((t - since) / COLLAPSE_SECS).clamp(0.0, 1.0);
                ((x - 0.6) / 0.4).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// Advance time-driven transitions. Collapsing -> EnterUser clears the
    /// username (it must be re-entered every attempt, by design).
    pub fn advance(&mut self, t: f64) {
        match self.phase {
            Phase::Growing { since } if t - since >= GROW_SECS => {
                self.phase = Phase::EnterPassword;
            }
            Phase::Failed { since } if t - since >= FAILED_HOLD_SECS => {
                self.phase = Phase::Collapsing { since: t };
            }
            Phase::Collapsing { since } if t - since >= COLLAPSE_SECS => {
                self.phase = Phase::EnterUser;
                self.username.clear();
            }
            _ => {}
        }
    }

    /// Username Enter. True -> the app sends CreateSession and the cube grows.
    pub fn user_submit(&mut self, t: f64) -> bool {
        if self.phase != Phase::EnterUser || self.username.trim().is_empty() {
            return false;
        }
        self.phase = Phase::Growing { since: t };
        true
    }

    /// Keys land in the password buffer only in these phases.
    pub fn accepts_password_input(&self) -> bool {
        matches!(self.phase, Phase::Growing { .. } | Phase::EnterPassword)
    }

    /// Password Enter. True -> the app sends PostAuth and the wave plays.
    pub fn password_submit(&mut self) -> bool {
        if !self.accepts_password_input() {
            return false;
        }
        self.phase = Phase::Validating;
        true
    }

    /// greetd said yes (possibly straight from CreateSession for a
    /// passwordless PAM stack). True -> the app sends StartSession.
    pub fn on_auth_ok(&mut self) -> bool {
        match self.phase {
            Phase::Growing { .. } | Phase::EnterPassword | Phase::Validating => {
                self.phase = Phase::Starting;
                true
            }
            _ => false,
        }
    }

    /// greetd said no: hold the wrong-flash, then collapse (via advance()).
    /// The app also clears the password, plays scene.wrong, and sends Cancel.
    pub fn on_auth_failed(&mut self, t: f64) {
        if matches!(
            self.phase,
            Phase::Growing { .. } | Phase::EnterPassword | Phase::Validating
        ) {
            self.phase = Phase::Failed { since: t };
        }
    }

    /// Esc with an empty password buffer (username-typo recovery), or the
    /// idle timeout: collapse back. The app also sends Cancel.
    pub fn cancel_to_user(&mut self, t: f64) {
        if matches!(self.phase, Phase::Growing { .. } | Phase::EnterPassword) {
            self.phase = Phase::Collapsing { since: t };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn to_password_phase(g: &mut Greeter) {
        g.username.push_str("voyd");
        assert!(g.user_submit(1.0));
        g.advance(1.0 + GROW_SECS + 1e-9);
        assert_eq!(g.phase, Phase::EnterPassword);
    }

    #[test]
    fn happy_path() {
        let mut g = Greeter::new();
        assert_eq!(g.cube_scale(0.0), 0.0);
        assert!(!g.user_submit(0.0)); // empty username refused
        to_password_phase(&mut g);
        assert_eq!(g.cube_scale(2.0), 1.0);
        assert!(g.password_submit());
        assert_eq!(g.phase, Phase::Validating);
        assert!(!g.password_submit()); // gated while validating
        assert!(g.on_auth_ok());
        assert_eq!(g.phase, Phase::Starting);
    }

    #[test]
    fn failure_collapses_and_clears_username() {
        let mut g = Greeter::new();
        to_password_phase(&mut g);
        g.password_submit();
        g.on_auth_failed(5.0);
        assert!(matches!(g.phase, Phase::Failed { .. }));
        g.advance(5.0 + FAILED_HOLD_SECS + 1e-9);
        assert!(matches!(g.phase, Phase::Collapsing { .. }));
        let t_collapse = 5.0 + FAILED_HOLD_SECS;
        // Mid-collapse the cube is shrinking and input is refused.
        let mid = g.cube_scale(t_collapse + COLLAPSE_SECS / 2.0);
        assert!(mid > 0.0 && mid < 1.0);
        assert!(!g.accepts_password_input());
        assert!(!g.password_submit());
        g.advance(t_collapse + COLLAPSE_SECS + 1e-9);
        assert_eq!(g.phase, Phase::EnterUser);
        assert!(g.username.is_empty(), "username must be re-entered every time");
    }

    #[test]
    fn grow_accepts_password_and_early_submit() {
        let mut g = Greeter::new();
        g.username.push_str("voyd");
        assert!(g.user_submit(1.0));
        // Mid-grow: typing goes to the password, Enter submits early.
        assert!(g.accepts_password_input());
        let s = g.cube_scale(1.0 + GROW_SECS / 2.0);
        assert!(s > 0.0 && s < 1.0);
        assert!(g.password_submit());
        assert_eq!(g.phase, Phase::Validating);
    }

    #[test]
    fn passwordless_auth_ok_from_growing() {
        let mut g = Greeter::new();
        g.username.push_str("kiosk");
        g.user_submit(0.0);
        assert!(g.on_auth_ok());
        assert_eq!(g.phase, Phase::Starting);
    }

    #[test]
    fn esc_cancel_returns_to_user() {
        let mut g = Greeter::new();
        to_password_phase(&mut g);
        g.cancel_to_user(10.0);
        assert!(matches!(g.phase, Phase::Collapsing { .. }));
        g.advance(10.0 + COLLAPSE_SECS + 1e-9);
        assert_eq!(g.phase, Phase::EnterUser);
    }

    #[test]
    fn box_alpha_envelope() {
        let mut g = Greeter::new();
        assert_eq!(g.box_alpha(0.0), 1.0);
        g.username.push_str("voyd");
        g.user_submit(0.0);
        assert!(g.box_alpha(GROW_SECS * 0.1) < 1.0);
        assert_eq!(g.box_alpha(GROW_SECS * 0.5), 0.0); // gone before grow ends
        g.advance(GROW_SECS + 1e-9);
        assert_eq!(g.box_alpha(GROW_SECS + 1.0), 0.0);
        g.cancel_to_user(2.0);
        assert_eq!(g.box_alpha(2.0), 0.0);
        assert!(g.box_alpha(2.0 + COLLAPSE_SECS * 0.9) > 0.0); // fading back in
    }

    #[test]
    fn auth_events_ignored_in_wrong_phases() {
        let mut g = Greeter::new();
        assert!(!g.on_auth_ok());
        g.on_auth_failed(0.0);
        assert_eq!(g.phase, Phase::EnterUser);
        g.cancel_to_user(0.0);
        assert_eq!(g.phase, Phase::EnterUser);
    }
}
