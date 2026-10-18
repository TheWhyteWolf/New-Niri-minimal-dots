// SPDX-License-Identifier: GPL-3.0-or-later
// Password-phase keysym table — copied from ../rainlock/src/input.rs (which
// follows swaylock's password.c). Only key_action/Action and the clear-timer
// constant come along: rainlock's indicator Machine is subsumed by the
// greeter's phase machine (state.rs). Fix keysym-table bugs in both.

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

/// swaylock's 10s clear-if-idle timeout for a partially typed password.
pub const PASSWORD_CLEAR_SECS: f64 = 10.0;

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
}
