// SPDX-License-Identifier: GPL-3.0-or-later
// raingreet — a greetd greeter (run under cage) that looks like rainlock: the
// wallpaper through a rainy window, a username prompt, then a password prompt
// where each keystroke is a drop landing on the glass.
//
// Everything but the look comes from LifeBranch's lifegreet: the password
// lives in an mlock'd buffer zeroed at every clear point, core dumps and
// ptrace are disabled, and PAM is greetd's job on the far side of
// $GREETD_SOCK.

mod app;
mod cli;
mod harden;
mod input;
mod ipc;
mod secure_buf;
mod sessions;
mod state;

fn main() {
    harden::harden_common();
    harden::refuse_setuid();
    harden::lock_all_memory();

    let cfg = cli::parse();

    let (events_tx, events_rx) = smithay_client_toolkit::reexports::calloop::channel::channel();

    // Debug-only stub authenticator, so the UI can run as a normal niri window
    // without greetd: password "ok" succeeds. Never in a release build.
    #[cfg(debug_assertions)]
    let ipc = if std::env::var_os("RAINGREET_TEST_NO_IPC").is_some() {
        eprintln!("raingreet: STUB AUTH (RAINGREET_TEST_NO_IPC) — password \"ok\" succeeds");
        ipc::spawn_stub(events_tx)
    } else {
        ipc::spawn(events_tx)
    };
    #[cfg(not(debug_assertions))]
    let ipc = ipc::spawn(events_tx);

    let code = app::run(cfg, ipc, events_rx);
    std::process::exit(code);
}
