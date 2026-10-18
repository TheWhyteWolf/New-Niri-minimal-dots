// SPDX-License-Identifier: GPL-3.0-or-later
// Two-process authentication, ported from swaylock's comm.c.
//
// The UI process never calls PAM. It spawns `/proc/self/exe --auth-child`
// (re-exec, not fork: no fork-safety hazards) BEFORE connecting to Wayland,
// and sends password-check requests over a pipe pair:
//
//   UI ──request──▶ fd 3 (child)     request = u32 LE length + that many bytes
//   UI ◀──reply──── fd 4 (child)     reply   = 1 verdict byte (1 = authed),
//                                    then u32 LE length + that many bytes of
//                                    PAM conversation text (empty on success;
//                                    e.g. pam_faillock's lockout notice)
//
// EOF on the request pipe tells the child to exit (UI death cleans it up);
// after a success reply the child exits on its own. pam_unix's ~2s failure
// delay blocks only the child while the UI keeps animating.

pub mod child;
pub mod parent;

/// Child-side fd numbers after the parent's dup2 in pre_exec.
pub const REQUEST_FD: i32 = 3;
pub const REPLY_FD: i32 = 4;

/// Reply-message size cap, enforced on both ends (child truncates, parent
/// treats anything larger as a broken channel).
pub const MAX_MSG: usize = 512;

pub const ARG_AUTH_CHILD: &str = "--auth-child";

/// Read exactly `buf.len()` bytes from fd. Returns false on EOF/error.
fn read_exact_fd(fd: i32, buf: &mut [u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::read(
                fd,
                buf[off..].as_mut_ptr() as *mut libc::c_void,
                buf.len() - off,
            )
        };
        if n > 0 {
            off += n as usize;
        } else if n < 0 && unsafe { *libc::__errno_location() } == libc::EINTR {
            continue;
        } else {
            return false;
        }
    }
    true
}

/// Write all of `buf` to fd. Returns false on error.
fn write_all_fd(fd: i32, buf: &[u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        let n = unsafe {
            libc::write(
                fd,
                buf[off..].as_ptr() as *const libc::c_void,
                buf.len() - off,
            )
        };
        if n > 0 {
            off += n as usize;
        } else if n < 0 && unsafe { *libc::__errno_location() } == libc::EINTR {
            continue;
        } else {
            return false;
        }
    }
    true
}
