// SPDX-License-Identifier: GPL-3.0-or-later
// Keyboard handling and the indicator state machines — swaylock parity
// (password.c:137-217 for the keysym table, swaylock.h for the states).
// Pure logic: the app layer owns timers and executes the returned actions.

use smithay_client_toolkit::seat::keyboard::Keysym;

/// What a keypress means. The app maps these onto buffer edits, scene
/// effects, timer arming, and auth submission.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// Append this UTF-8 text to the password buffer.
    Append(String),
    /// Delete one codepoint (Backspace/Delete without Ctrl).
    PopChar,
    /// Clear the whole buffer (Esc, Ctrl+U, Ctrl+C, Ctrl+Backspace/Delete).
    ClearAll,
    /// Submit the buffer for authentication (Return, KP_Enter, Ctrl+m/j/d).
    Submit,
    /// A bare modifier went down (Shift, Ctrl, Caps, Alt, Super).
    Neutral,
    /// Nothing relevant (unmapped key with no text).
    Ignore,
}

pub fn key_action(keysym: Keysym, utf8: Option<&str>, ctrl: bool) -> Action {
    // With Caps Lock on, letter keys report their uppercase keysym, so match
    // both cases for the Ctrl-chord shortcuts (Ctrl+U to clear must still work
    // with caps lock engaged).
    match keysym {
        Keysym::Return | Keysym::KP_Enter => Action::Submit,
        Keysym::m | Keysym::j | Keysym::d | Keysym::M | Keysym::J | Keysym::D if ctrl => {
            Action::Submit
        }
        Keysym::BackSpace | Keysym::Delete | Keysym::KP_Delete => {
            if ctrl {
                Action::ClearAll
            } else {
                Action::PopChar
            }
        }
        Keysym::Escape => Action::ClearAll,
        Keysym::u | Keysym::c | Keysym::U | Keysym::C if ctrl => Action::ClearAll,
        Keysym::Caps_Lock
        | Keysym::Shift_L
        | Keysym::Shift_R
        | Keysym::Control_L
        | Keysym::Control_R
        | Keysym::Meta_L
        | Keysym::Meta_R
        | Keysym::Alt_L
        | Keysym::Alt_R
        | Keysym::Super_L
        | Keysym::Super_R => Action::Neutral,
        _ => match utf8 {
            Some(s) if !s.is_empty() && !ctrl => Action::Append(s.to_string()),
            _ => Action::Ignore,
        },
    }
}

/// swaylock's input_state: drives the short-lived visual reactions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InputState {
    Idle,
    Clear,
    Letter,
    Backspace,
    Neutral,
}

/// swaylock's auth_state: drives verifying/wrong visuals and submit gating.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AuthState {
    Idle,
    Validating,
    Invalid,
}

/// Timers the app must (re)arm after handling an event. Durations are
/// swaylock's: 1.5s input-idle, 3s auth-invalid reset, 10s password clear.
#[derive(Debug, PartialEq, Default)]
pub struct TimerOps {
    pub arm_input_idle: bool,   // -> InputState::Idle after 1.5s
    pub arm_password_clear: bool, // -> clear buffer after 10s
    pub cancel_password_clear: bool,
}

pub const INPUT_IDLE_SECS: f64 = 1.5;
pub const AUTH_INVALID_SECS: f64 = 3.0;
pub const PASSWORD_CLEAR_SECS: f64 = 10.0;

pub struct Machine {
    pub input: InputState,
    pub auth: AuthState,
    pub failed_attempts: u32,
}

impl Machine {
    pub fn new() -> Machine {
        Machine { input: InputState::Idle, auth: AuthState::Idle, failed_attempts: 0 }
    }

    /// A letter was appended.
    pub fn on_letter(&mut self) -> TimerOps {
        self.input = InputState::Letter;
        TimerOps { arm_input_idle: true, arm_password_clear: true, ..Default::default() }
    }

    /// Backspace removed a character; `now_empty` = buffer emptied by it.
    pub fn on_backspace(&mut self, now_empty: bool) -> TimerOps {
        if now_empty {
            self.on_clear()
        } else {
            self.input = InputState::Backspace;
            TimerOps { arm_input_idle: true, arm_password_clear: true, ..Default::default() }
        }
    }

    /// Buffer cleared (Esc/Ctrl+U/Ctrl+C/Ctrl+BS/emptied/10s timer).
    pub fn on_clear(&mut self) -> TimerOps {
        self.input = InputState::Clear;
        TimerOps { arm_input_idle: true, cancel_password_clear: true, ..Default::default() }
    }

    /// A bare modifier went down.
    pub fn on_neutral(&mut self) -> TimerOps {
        self.input = InputState::Neutral;
        TimerOps { arm_input_idle: true, arm_password_clear: true, ..Default::default() }
    }

    /// Submit requested. Returns whether the submission should proceed
    /// (blocked while validating; empty submits are gated by the caller
    /// because only it can see the buffer).
    pub fn on_submit(&mut self) -> (bool, TimerOps) {
        if self.auth == AuthState::Validating {
            return (false, TimerOps::default());
        }
        self.input = InputState::Idle;
        self.auth = AuthState::Validating;
        (true, TimerOps { cancel_password_clear: true, ..Default::default() })
    }

    /// Auth verdict arrived. Returns true if the session should unlock.
    pub fn on_reply(&mut self, ok: bool) -> bool {
        if ok {
            return true;
        }
        self.auth = AuthState::Invalid;
        self.failed_attempts += 1;
        false
    }

    /// 1.5s input-idle timer fired.
    pub fn on_input_idle(&mut self) {
        self.input = InputState::Idle;
    }

    /// 3s auth-invalid timer fired.
    pub fn on_auth_idle(&mut self) {
        if self.auth == AuthState::Invalid {
            self.auth = AuthState::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysym_table() {
        assert_eq!(key_action(Keysym::Return, Some("\r"), false), Action::Submit);
        assert_eq!(key_action(Keysym::KP_Enter, None, false), Action::Submit);
        assert_eq!(key_action(Keysym::m, Some("m"), true), Action::Submit);
        assert_eq!(key_action(Keysym::m, Some("m"), false), Action::Append("m".into()));
        assert_eq!(key_action(Keysym::BackSpace, None, false), Action::PopChar);
        assert_eq!(key_action(Keysym::BackSpace, None, true), Action::ClearAll);
        assert_eq!(key_action(Keysym::Delete, None, true), Action::ClearAll);
        assert_eq!(key_action(Keysym::Escape, None, false), Action::ClearAll);
        assert_eq!(key_action(Keysym::u, Some("u"), true), Action::ClearAll);
        assert_eq!(key_action(Keysym::c, Some("c"), true), Action::ClearAll);
        // Caps Lock on: letter keys report uppercase syms — Ctrl chords still work.
        assert_eq!(key_action(Keysym::U, Some("U"), true), Action::ClearAll);
        assert_eq!(key_action(Keysym::C, Some("C"), true), Action::ClearAll);
        assert_eq!(key_action(Keysym::M, Some("M"), true), Action::Submit);
        assert_eq!(key_action(Keysym::Shift_L, None, false), Action::Neutral);
        assert_eq!(key_action(Keysym::a, Some("a"), false), Action::Append("a".into()));
        // Ctrl+letter combos that aren't bound: swallowed, not typed.
        assert_eq!(key_action(Keysym::a, Some("a"), true), Action::Ignore);
        assert_eq!(key_action(Keysym::F5, None, false), Action::Ignore);
    }

    #[test]
    fn submit_gated_while_validating() {
        let mut m = Machine::new();
        let (go, _) = m.on_submit();
        assert!(go);
        assert_eq!(m.auth, AuthState::Validating);
        let (go2, _) = m.on_submit();
        assert!(!go2);
    }

    #[test]
    fn wrong_reply_counts_and_resets() {
        let mut m = Machine::new();
        m.on_submit();
        assert!(!m.on_reply(false));
        assert_eq!(m.auth, AuthState::Invalid);
        assert_eq!(m.failed_attempts, 1);
        m.on_auth_idle();
        assert_eq!(m.auth, AuthState::Idle);
        m.on_submit();
        assert!(m.on_reply(true));
    }

    #[test]
    fn backspace_to_empty_is_clear() {
        let mut m = Machine::new();
        let ops = m.on_backspace(true);
        assert_eq!(m.input, InputState::Clear);
        assert!(ops.cancel_password_clear);
        let ops = m.on_backspace(false);
        assert_eq!(m.input, InputState::Backspace);
        assert!(ops.arm_password_clear);
    }
}
