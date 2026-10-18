// SPDX-License-Identifier: GPL-3.0-or-later
// Process hardening. Runs in BOTH processes before any input is read.
// swaylock does none of this (its protection is isolation + mlock alone);
// core-dump and ptrace denial close the gap.

/// Disable core dumps and mark the process undumpable (blocks ptrace and
/// /proc/<pid>/mem access from non-root peers). Both processes call this
/// first thing; the auth child must repeat it because execve resets the
/// dumpable flag.
pub fn harden_common() {
    unsafe {
        let rl = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if libc::setrlimit(libc::RLIMIT_CORE, &rl) != 0 {
            eprintln!("rainlock: warning: could not disable core dumps");
        }
        if libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) != 0 {
            eprintln!("rainlock: warning: could not set PR_SET_DUMPABLE=0");
        }
    }
}

/// The PAM backend must not run setuid (parity with swaylock pam.c): under
/// PAM there is nothing to be privileged for, and an unexpected setuid bit
/// means someone made a mistake worth refusing to run with.
pub fn refuse_setuid() {
    unsafe {
        if libc::getuid() != libc::geteuid() || libc::getgid() != libc::getegid() {
            eprintln!("rainlock: refusing to run setuid/setgid; run `chmod a-s` on the binary");
            std::process::exit(1);
        }
    }
}

/// Abort before locking if the PAM service file is missing. Without it,
/// pam_start("rainlock") falls through to /etc/pam.d/other (pam_deny on
/// Arch): the session would lock but provably never unlock.
pub fn require_pam_service() {
    const PATH: &str = "/etc/pam.d/rainlock";
    if !std::path::Path::new(PATH).exists() {
        eprintln!(
            "rainlock: {PATH} is missing — refusing to lock a session that could never unlock.\n\
             Install it first:  bash rainglass/install.sh"
        );
        std::process::exit(1);
    }
}

/// Auth child only: pin every current and future page so password material
/// can't be swapped out. MCL_FUTURE under a low RLIMIT_MEMLOCK is a trap
/// (see LifeBranch lifegreet/src/harden.rs — it killed that greeter at boot): mlockall
/// succeeds while the process is small, then later mmaps fail EAGAIN. The
/// child is tiny but PAM still dlopens modules, and an auth child that can't
/// authenticate is a session that never unlocks — so below the threshold,
/// lock only what is already mapped. That leaves a gap: this process holds
/// the password in ordinary heap copies (the request Vec, the CString handed
/// to PAM), and heap pages mapped after this call are not locked. The UI's
/// SecureBuf is mlocked, but it lives in the other process. Raising
/// RLIMIT_MEMLOCK to FULL_LOCK_MIN closes the gap.
pub fn lock_all_memory() {
    // The auth child stays small: PAM modules + libc, well under 64 MiB.
    const FULL_LOCK_MIN: libc::rlim_t = 64 * 1024 * 1024;
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
                    "rainlock: warning: RLIMIT_MEMLOCK is {} KiB — not arming MCL_FUTURE \
                     (password copies in the auth child may be swappable)",
                    rl.rlim_cur / 1024
                );
                return;
            }
        }
        if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) != 0 {
            eprintln!("rainlock: warning: mlockall failed (password copies in the auth child may be swappable)");
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
