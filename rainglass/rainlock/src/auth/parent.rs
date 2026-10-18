// SPDX-License-Identifier: GPL-3.0-or-later
// UI-side of the auth channel: spawn the child, send requests, expose the
// reply fd for the event loop to poll.

use super::{write_all_fd, ARG_AUTH_CHILD, MAX_MSG, REPLY_FD, REQUEST_FD};
use crate::secure_buf::SecureBuf;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub struct AuthProc {
    child: Child,
    request_w: i32,
    reply_r: i32,
}

fn pipe2_cloexec() -> (i32, i32) {
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        eprintln!("rainlock: failed to create auth pipes");
        std::process::exit(1);
    }
    (fds[0], fds[1])
}

impl AuthProc {
    /// Spawn `/proc/self/exe --auth-child` with the pipe ends dup2'd onto
    /// fds 3/4 (dup2 clears O_CLOEXEC on the copies; every other descriptor
    /// stays close-on-exec). Call BEFORE any Wayland connection exists.
    pub fn spawn() -> AuthProc {
        let (req_r, req_w) = pipe2_cloexec();
        let (rep_r, rep_w) = pipe2_cloexec();

        let mut cmd = Command::new("/proc/self/exe");
        cmd.arg(ARG_AUTH_CHILD);
        unsafe {
            cmd.pre_exec(move || {
                // Move the pipe ends out of the [3,4] target range first
                // (F_DUPFD_CLOEXEC picks fds >= 10), then dup2 them down.
                // dup2 clears O_CLOEXEC on the destination, so fds 3/4 survive
                // exec; the >=10 copies and the cloexec originals do not.
                // Going through temporaries avoids clobbering if a pipe end
                // already sits on fd 3 or 4.
                let tmp_r = libc::fcntl(req_r, libc::F_DUPFD_CLOEXEC, 10);
                let tmp_w = libc::fcntl(rep_w, libc::F_DUPFD_CLOEXEC, 10);
                if tmp_r < 0 || tmp_w < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::dup2(tmp_r, REQUEST_FD) < 0 || libc::dup2(tmp_w, REPLY_FD) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(tmp_r);
                libc::close(tmp_w);
                Ok(())
            });
        }
        let child = cmd.spawn().unwrap_or_else(|e| {
            eprintln!("rainlock: failed to spawn auth child: {e}");
            std::process::exit(1);
        });

        // Close the child's ends in this process.
        unsafe {
            libc::close(req_r);
            libc::close(rep_w);
        }

        AuthProc {
            child,
            request_w: req_w,
            reply_r: rep_r,
        }
    }

    /// The fd the event loop polls for the 1-byte auth verdict.
    pub fn reply_fd(&self) -> i32 {
        self.reply_r
    }

    /// Read the pending verdict + PAM conversation text (empty on success or
    /// when PAM had nothing to say). Call only when reply_fd is readable; the
    /// child writes the whole reply at once, so the follow-up reads don't
    /// block meaningfully. None = channel broke (child died / protocol junk).
    pub fn read_reply(&mut self) -> Option<(bool, Option<String>)> {
        let mut b = [0u8; 1];
        if !super::read_exact_fd(self.reply_r, &mut b) {
            return None;
        }
        let mut len_buf = [0u8; 4];
        if !super::read_exact_fd(self.reply_r, &mut len_buf) {
            return None;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len > MAX_MSG {
            return None; // not our protocol — treat as a dead child
        }
        let msg = if len > 0 {
            let mut buf = vec![0u8; len];
            if !super::read_exact_fd(self.reply_r, &mut buf) {
                return None;
            }
            Some(String::from_utf8_lossy(&buf).into_owned())
        } else {
            None
        };
        Some((b[0] == 1, msg))
    }

    /// Send the password for verification. The caller clears the buffer
    /// immediately after (swaylock parity: cleared whether or not the write
    /// succeeded). Returns false if the channel broke.
    pub fn submit(&mut self, pw: &SecureBuf) -> bool {
        let len = (pw.len() as u32).to_le_bytes();
        write_all_fd(self.request_w, &len) && write_all_fd(self.request_w, pw.as_bytes())
    }
}

impl Drop for AuthProc {
    fn drop(&mut self) {
        // Closing the request pipe is the child's exit signal.
        unsafe {
            libc::close(self.request_w);
            libc::close(self.reply_r);
        }
        let _ = self.child.wait();
    }
}
