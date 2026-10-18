// SPDX-License-Identifier: GPL-3.0-or-later
// Copied verbatim from ../rainlock/src/secure_buf.rs (same workspace). Fix bugs in both.
// The password buffer: one anonymous mmap'd page, mlocked, excluded from
// core dumps, volatile-zeroed on every clear. Ported from swaylock's
// password-buffer.c with the clearing discipline of password.c.

use std::ptr;

/// Max password payload. One page holds it comfortably; input beyond the cap
/// is silently dropped (swaylock parity — password.c append_ch).
pub const MAX_LEN: usize = 1023;

pub struct SecureBuf {
    ptr: *mut u8,
    page: usize,
    len: usize,
    locked: bool,
}

// The raw pointer is owned uniquely by this struct.
unsafe impl Send for SecureBuf {}

impl SecureBuf {
    pub fn new() -> SecureBuf {
        unsafe {
            let page = libc::sysconf(libc::_SC_PAGESIZE).max(4096) as usize;
            let ptr = libc::mmap(
                ptr::null_mut(),
                page,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            );
            if ptr == libc::MAP_FAILED {
                eprintln!("raingreet: failed to allocate password buffer");
                std::process::exit(1);
            }
            let ptr = ptr as *mut u8;

            // mlock: retry EAGAIN a few times; tolerate EPERM (no privilege)
            // but keep zeroizing discipline regardless (swaylock parity).
            let mut locked = false;
            for attempt in 0..5 {
                if libc::mlock(ptr as *const libc::c_void, page) == 0 {
                    locked = true;
                    break;
                }
                let err = *libc::__errno_location();
                if err == libc::EAGAIN {
                    if attempt == 4 {
                        eprintln!("raingreet: warning: mlock kept returning EAGAIN — password buffer may be swappable");
                    }
                    continue;
                }
                if err == libc::EPERM {
                    eprintln!("raingreet: warning: unable to mlock password buffer (EPERM)");
                } else {
                    eprintln!("raingreet: warning: mlock failed (errno {err})");
                }
                break;
            }

            // Never let this page into a core dump even if RLIMIT_CORE were raised.
            libc::madvise(ptr as *mut libc::c_void, page, libc::MADV_DONTDUMP);

            SecureBuf {
                ptr,
                page,
                len: 0,
                locked,
            }
        }
    }

    #[allow(dead_code)] // the greeter uses a subset of the rainlock API
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    /// Append UTF-8 text; silently drops input past MAX_LEN (never truncates
    /// mid-codepoint: all-or-nothing per call, like swaylock's append_ch).
    pub fn push_str(&mut self, s: &str) {
        let b = s.as_bytes();
        if b.is_empty() || self.len + b.len() > MAX_LEN {
            return;
        }
        unsafe {
            ptr::copy_nonoverlapping(b.as_ptr(), self.ptr.add(self.len), b.len());
        }
        self.len += b.len();
    }

    /// Remove the last UTF-8 codepoint, zeroizing its bytes.
    /// Returns false if the buffer was already empty.
    pub fn pop_char(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        let bytes = self.as_bytes();
        let mut start = self.len - 1;
        // Walk back over UTF-8 continuation bytes (0b10xxxxxx).
        while start > 0 && bytes[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        unsafe {
            for i in start..self.len {
                ptr::write_volatile(self.ptr.add(i), 0);
            }
        }
        self.len = start;
        true
    }

    /// Volatile-zero the whole page (not just `len` bytes).
    pub fn clear(&mut self) {
        unsafe {
            for i in 0..self.page {
                ptr::write_volatile(self.ptr.add(i), 0);
            }
        }
        self.len = 0;
    }
}

impl Drop for SecureBuf {
    fn drop(&mut self) {
        self.clear();
        unsafe {
            if self.locked {
                libc::munlock(self.ptr as *const libc::c_void, self.page);
            }
            libc::munmap(self.ptr as *mut libc::c_void, self.page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_multibyte() {
        let mut b = SecureBuf::new();
        b.push_str("aé🔒");
        assert_eq!(b.as_bytes(), "aé🔒".as_bytes());
        assert!(b.pop_char());
        assert_eq!(b.as_bytes(), "aé".as_bytes());
        assert!(b.pop_char());
        assert_eq!(b.as_bytes(), b"a");
        assert!(b.pop_char());
        assert!(b.is_empty());
        assert!(!b.pop_char());
    }

    #[test]
    fn cap_drops_silently() {
        let mut b = SecureBuf::new();
        let big = "x".repeat(MAX_LEN);
        b.push_str(&big);
        assert_eq!(b.len(), MAX_LEN);
        b.push_str("y"); // over cap: dropped whole
        assert_eq!(b.len(), MAX_LEN);
    }

    #[test]
    fn clear_really_clears() {
        let mut b = SecureBuf::new();
        b.push_str("hunter2");
        let p = b.ptr;
        let page = b.page;
        b.clear();
        assert!(b.is_empty());
        // Read the raw page back: every byte must be zero.
        let raw = unsafe { std::slice::from_raw_parts(p, page) };
        assert!(raw.iter().all(|&x| x == 0));
    }

    #[test]
    fn pop_zeroizes_tail() {
        let mut b = SecureBuf::new();
        b.push_str("ab");
        b.pop_char();
        let raw = unsafe { std::slice::from_raw_parts(b.ptr, 4) };
        assert_eq!(raw[0], b'a');
        assert_eq!(raw[1], 0);
    }
}
