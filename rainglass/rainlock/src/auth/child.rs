// SPDX-License-Identifier: GPL-3.0-or-later
// The auth child: a tiny PAM-only process. Ported from swaylock's pam.c.
// It never touches Wayland, fonts, or the scene — it reads password requests
// from fd 3, runs pam_authenticate, and writes a verdict byte to fd 4.

use super::{read_exact_fd, write_all_fd, MAX_MSG, REPLY_FD, REQUEST_FD};
use crate::harden;
use pam_client::{Context, ConversationHandler, ErrorCode, Flag};
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::rc::Rc;
use zeroize::Zeroize;

const SERVICE: &str = "rainlock";

/// Hands the password to PAM exactly once per authenticate() and aborts the
/// conversation on any second prompt — the systemd-homed internal-retry guard
/// from swaylock pam.c:43-57. Info/error text (e.g. pam_faillock's "account
/// locked" notice) is collected for the UI; the Rc lets us read it back after
/// the Context has consumed the handler.
struct OneShot {
    password: Option<CString>,
    msgs: Rc<RefCell<Vec<String>>>,
}

impl ConversationHandler for OneShot {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        self.take()
    }
    fn prompt_echo_off(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        self.take()
    }
    fn text_info(&mut self, msg: &CStr) {
        self.push(msg);
    }
    fn error_msg(&mut self, msg: &CStr) {
        self.push(msg);
    }
}

impl OneShot {
    fn take(&mut self) -> Result<CString, ErrorCode> {
        match self.password.take() {
            Some(pw) => Ok(pw),
            None => Err(ErrorCode::ABORT), // second prompt in one authenticate
        }
    }

    fn push(&mut self, msg: &CStr) {
        // Control chars would confuse the renderer; passwords never appear in
        // conversation text, so plain (bounded) collection is safe.
        let s: String = msg
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let s = s.trim().to_string();
        if !s.is_empty() {
            self.msgs.borrow_mut().push(s);
        }
    }
}

impl Drop for OneShot {
    fn drop(&mut self) {
        if let Some(pw) = self.password.take() {
            // Only reached if PAM never consumed it (e.g. pam_start failure).
            pw.into_bytes().zeroize();
        }
    }
}

fn username() -> String {
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if pw.is_null() || (*pw).pw_name.is_null() {
            eprintln!("rainlock: auth-child: getpwuid({uid}) failed");
            std::process::exit(1);
        }
        CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned()
    }
}

/// Entry point; never returns. Called before anything else in main().
pub fn run() -> ! {
    harden::harden_common(); // execve reset the dumpable flag — set it again
    harden::refuse_setuid();
    harden::lock_all_memory();

    // SIGUSR1 gracefully unlocks the UI process; `pkill -USR1 rainlock`
    // matches this process by name too, so it must shrug the signal off
    // (swaylock comm.c:100-103).
    unsafe {
        libc::signal(libc::SIGUSR1, libc::SIG_IGN);
    }

    let user = username();

    loop {
        // Request: u32 LE length, then that many password bytes.
        let mut len_buf = [0u8; 4];
        if !read_exact_fd(REQUEST_FD, &mut len_buf) {
            std::process::exit(0); // EOF: UI is gone
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > crate::secure_buf::MAX_LEN {
            eprintln!("rainlock: auth-child: oversized request");
            std::process::exit(1);
        }
        let mut pw_bytes = vec![0u8; len];
        if !read_exact_fd(REQUEST_FD, &mut pw_bytes) {
            pw_bytes.zeroize();
            std::process::exit(0);
        }

        // CString::new copies; zeroize our copy immediately after.
        let cpw = match CString::new(pw_bytes.clone()) {
            Ok(c) => c,
            Err(e) => {
                // Interior NUL can't be a valid password; report failure.
                // NulError owns the moved-in clone — zeroize it too, not just
                // the original, so no password copy is left in freed heap.
                e.into_vec().zeroize();
                pw_bytes.zeroize();
                if !write_all_fd(REPLY_FD, &[0]) {
                    std::process::exit(1);
                }
                continue;
            }
        };
        pw_bytes.zeroize();

        let (ok, msg) = authenticate(&user, cpw);

        // Reply: verdict byte, then the (possibly empty) conversation text —
        // u32 LE length + bytes — so the UI can show e.g. faillock lockouts.
        let msg_bytes = &msg.as_bytes()[..msg.len().min(MAX_MSG)];
        let len = (msg_bytes.len() as u32).to_le_bytes();
        if !(write_all_fd(REPLY_FD, &[ok as u8])
            && write_all_fd(REPLY_FD, &len)
            && write_all_fd(REPLY_FD, msg_bytes))
        {
            std::process::exit(1);
        }
        if ok {
            // Don't process requests queued after a success (pam.c:133-137).
            std::process::exit(0);
        }
    }
}

fn authenticate(user: &str, password: CString) -> (bool, String) {
    let msgs = Rc::new(RefCell::new(Vec::new()));
    let conv = OneShot {
        password: Some(password),
        msgs: Rc::clone(&msgs),
    };
    let mut ctx = match Context::new(SERVICE, Some(user), conv) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rainlock: auth-child: pam_start failed: {e}");
            return (false, String::new());
        }
    };

    // pam_unix applies its ~2s randomized failure delay inside this call —
    // deliberately kept (brute-force resistance); only this process blocks.
    let ok = match ctx.authenticate(Flag::NONE) {
        Ok(()) => true,
        Err(e) => {
            eprintln!("rainlock: auth-child: authentication failed: {e}");
            false
        }
    };

    // Refresh credentials (parity with swaylock pam.c:140; e.g. renews
    // Kerberos tickets where configured). Failure is non-fatal.
    let _ = ctx.reinitialize_credentials(Flag::NONE);

    // Only failures carry text to the UI: modules like pam_faillock explain
    // themselves through the conversation ("account locked, N minutes left"),
    // which beats the generic wrong-flash. A plain bad password logs nothing.
    let msg = if ok {
        String::new()
    } else {
        msgs.borrow().join(" · ")
    };
    if !msg.is_empty() {
        eprintln!("rainlock: auth-child: pam: {msg}");
    }
    (ok, msg)
    // Context drop runs pam_end.
}
