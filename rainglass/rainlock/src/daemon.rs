// SPDX-License-Identifier: GPL-3.0-or-later
// Early-fork daemonize (-f), ported from swaylock's ordering guarantee.
//
// With -f we fork BEFORE connecting to Wayland (single-threaded, no locks —
// fork is trivially safe). The parent blocks on a status pipe and exits 0
// only once the child reports the `locked` event; the child continues as the
// real locker. This is what makes `rainlock -f && systemctl suspend` and
// swayidle's `-w before-sleep` race-free: the shell command doesn't return
// until the screen is confirmed locked.

use std::sync::atomic::{AtomicI32, Ordering};

// Write end of the status pipe, inherited by the child across the fork.
static STATUS_W: AtomicI32 = AtomicI32::new(-1);

/// Fork for daemonization. In the parent: block until the child signals, then
/// exit with 0 (locked) or 1 (failed). In the child: setsid, remember the
/// status fd, and return so it becomes the real locker.
pub fn fork_early() {
    let mut fds = [0i32; 2];
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            eprintln!("rainlock: daemonize pipe failed");
            std::process::exit(1);
        }
        let pid = libc::fork();
        if pid < 0 {
            eprintln!("rainlock: fork failed");
            std::process::exit(1);
        }
        if pid > 0 {
            // Parent: wait for the child's one status byte.
            libc::close(fds[1]);
            let mut b = [0u8; 1];
            let n = libc::read(fds[0], b.as_mut_ptr() as *mut libc::c_void, 1);
            if n == 1 && b[0] == 1 {
                std::process::exit(0); // locked OK
            }
            std::process::exit(1); // child died or reported failure
        }
        // Child.
        libc::close(fds[0]);
        STATUS_W.store(fds[1], Ordering::SeqCst);
        libc::setsid();
    }
}

/// Child: called from the `locked` handler. Report success to the parent and
/// detach stdio so the terminal is freed.
pub fn finish_daemonize() {
    let fd = STATUS_W.swap(-1, Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    unsafe {
        let _ = libc::write(fd, [1u8].as_ptr() as *const libc::c_void, 1);
        libc::close(fd);
        redirect_stdio_to_null();
    }
}

/// Child: called from the `finished` handler. Report failure so the parent's
/// `&&` chain halts and it exits nonzero.
pub fn abort_daemonize() {
    let fd = STATUS_W.swap(-1, Ordering::SeqCst);
    if fd < 0 {
        return;
    }
    unsafe {
        let _ = libc::write(fd, [0u8].as_ptr() as *const libc::c_void, 1);
        libc::close(fd);
    }
}

unsafe fn redirect_stdio_to_null() {
    let devnull = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
    if devnull >= 0 {
        libc::dup2(devnull, libc::STDIN_FILENO);
        libc::dup2(devnull, libc::STDOUT_FILENO);
        // Keep stderr: warnings still reach the journal.
        if devnull > libc::STDERR_FILENO {
            libc::close(devnull);
        }
    }
}
