// SPDX-License-Identifier: GPL-3.0-or-later
// Process hardening — subset of ../rainlock/src/harden.rs (fix bugs in both).
// The greeter runs as the unprivileged `greeter` user, but a typed password
// still transits this process: keep the core-dump/ptrace/swap protections.
// require_pam_service is dropped — greetd owns PAM on the other side of the
// IPC socket.

/// Disable core dumps and mark the process undumpable (blocks ptrace and
/// /proc/<pid>/mem access from non-root peers).
pub fn harden_common() {
    unsafe {
        let rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::setrlimit(libc::RLIMIT_CORE, &rl) != 0 {
            eprintln!("raingreet: warning: could not disable core dumps");
        }
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            eprintln!("raingreet: warning: could not set PR_SET_DUMPABLE=0");
        }
    }
}

/// There is nothing to be privileged for; an unexpected setuid bit means
/// someone made a mistake worth refusing to run with.
pub fn refuse_setuid() {
    unsafe {
        if libc::getuid() != libc::geteuid() || libc::getgid() != libc::getegid() {
            eprintln!("raingreet: refusing to run setuid/setgid; run `chmod a-s` on the binary");
            std::process::exit(1);
        }
    }
}

/// Pin every current and future page so password material can't be swapped
/// out — but only when RLIMIT_MEMLOCK can hold the whole greeter. MCL_FUTURE
/// under a low ceiling is a trap: mlockall succeeds while the process is
/// still small, then every later mmap counts against the limit and fails
/// with EAGAIN once it's crossed. Under greetd's systemd default of 8 MiB
/// that's exactly what happened at boot — the SHM frame pool (8.3 MiB at
/// 1080p alone; ~112 MiB locked in total) could never map, and the greeter
/// died before drawing anything. Below the threshold, lock only what is
/// already mapped; the per-buffer mlock in SecureBuf is the real backstop.
/// greeter-install.sh raises the service limit (LimitMEMLOCK=infinity) so
/// the full lock engages on a proper install.
pub fn lock_all_memory() {
    // Frame buffers scale with the output: ~112 MiB locked at 1080p,
    // ~170 MiB at 4K/Retina. Demand real headroom before arming MCL_FUTURE.
    const FULL_LOCK_MIN: libc::rlim_t = 512 * 1024 * 1024;
    unsafe {
        let mut rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::getrlimit(libc::RLIMIT_MEMLOCK, &mut rl) == 0 {
            if rl.rlim_cur < rl.rlim_max {
                // Take whatever headroom the hard limit already allows.
                let want = libc::rlimit {
                    rlim_cur: rl.rlim_max,
                    rlim_max: rl.rlim_max,
                };
                if libc::setrlimit(libc::RLIMIT_MEMLOCK, &want) == 0 {
                    rl.rlim_cur = rl.rlim_max;
                }
            }
            if rl.rlim_cur < FULL_LOCK_MIN {
                // MCL_CURRENT only: locks what exists now, never poisons a
                // future mmap. May itself fail if the limit is tiny — fine.
                libc::mlockall(libc::MCL_CURRENT);
                eprintln!(
                    "raingreet: warning: RLIMIT_MEMLOCK is {} KiB — not arming MCL_FUTURE \
                     (password buffer is still mlocked); raise greetd's LimitMEMLOCK",
                    rl.rlim_cur / 1024
                );
                return;
            }
        }
        if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) != 0 {
            eprintln!("raingreet: warning: mlockall failed (password buffer is still mlocked)");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn hardening_syscalls_succeed() {
        super::harden_common();
        unsafe {
            let mut rl = libc::rlimit {
                rlim_cur: 99,
                rlim_max: 99,
            };
            assert_eq!(libc::getrlimit(libc::RLIMIT_CORE, &mut rl), 0);
            assert_eq!(rl.rlim_cur, 0);
            assert_eq!(libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0), 0);
        }
    }
}
