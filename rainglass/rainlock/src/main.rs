// SPDX-License-Identifier: GPL-3.0-or-later
// rainlock — an ext-session-lock screen locker for niri: your wallpaper seen
// through a rainy window.
//
// The rain is cosmetic. Everything security-relevant comes unchanged from
// LifeBranch's lifelock, which modelled it on swaylock 1.8.6 (see ../NOTICE):
// the auth child runs PAM in a separate process, the password lives in an
// mlock'd buffer that is zeroed at every clear point, core dumps and ptrace
// are disabled, and the session is only ever unlocked after PAM says yes.

mod app;
mod auth;
mod cli;
mod daemon;
mod harden;
mod input;
mod secure_buf;

fn main() {
    // The auth child is a distinct mode of this same binary. Dispatch it as
    // the very first thing — before CLI parsing, fonts, the GPU or Wayland —
    // so the child process never links any of that into its failure domain.
    if std::env::args().nth(1).as_deref() == Some(auth::ARG_AUTH_CHILD) {
        auth::child::run(); // never returns
    }

    // Debug-only harness: spawn the auth child, read a password from stdin,
    // print the verdict. Compiled ONLY in debug builds so this plaintext-stdin
    // path can never exist in a released/installed binary.
    #[cfg(debug_assertions)]
    if std::env::args().nth(1).as_deref() == Some("--selftest-auth") {
        selftest_auth();
        return;
    }

    // UI process.
    harden::harden_common();
    harden::refuse_setuid();

    let cfg = cli::parse();

    // A caller-supplied --ready-fd would otherwise be inherited by the auth
    // child across execve; mark it close-on-exec so only this process holds it.
    if let Some(fd) = cfg.ready_fd {
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFD);
            if flags >= 0 {
                libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
            }
        }
    }

    // Refuse to lock a session we could provably never unlock. A debug-only
    // env bypass lets the Wayland/render path be smoke-tested in a nested
    // compositor before the PAM service file is installed; it can never exist
    // in a release build.
    #[cfg(debug_assertions)]
    let skip_pam_check = std::env::var_os("RAINLOCK_TEST_NO_PAM").is_some();
    #[cfg(not(debug_assertions))]
    let skip_pam_check = false;
    if !skip_pam_check {
        harden::require_pam_service();
    }

    // Daemonize BEFORE spawning the auth child or touching Wayland, so the
    // surviving (child) process owns the auth pipes and display connection.
    if cfg.daemonize {
        daemon::fork_early();
    }

    // Spawn the auth child before connecting to Wayland (swaylock ordering).
    let auth = auth::parent::AuthProc::spawn();

    let code = app::run(cfg, auth);
    std::process::exit(code);
}

#[cfg(debug_assertions)]
fn selftest_auth() {
    use std::io::Read;
    harden::harden_common();
    let mut auth = auth::parent::AuthProc::spawn();
    let mut pw = secure_buf::SecureBuf::new();
    let mut line = String::new();
    std::io::stdin().read_to_string(&mut line).ok();
    pw.push_str(line.trim_end_matches('\n'));
    line.clear();
    eprintln!("selftest: submitting {} byte password", pw.len());
    if !auth.submit(&pw) {
        eprintln!("selftest: submit failed (channel broke)");
        std::process::exit(3);
    }
    pw.clear();
    let fd = auth.reply_fd();
    let mut b = [0u8; 1];
    let n = unsafe { libc::read(fd, b.as_mut_ptr() as *mut libc::c_void, 1) };
    match (n, b[0]) {
        (1, 1) => println!("AUTH OK"),
        (1, _) => println!("AUTH FAIL"),
        _ => println!("AUTH ERROR (no reply)"),
    }
}
