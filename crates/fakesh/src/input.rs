//! stdin and SIGINT. The terminal is left in cooked mode, so a `read` returns one line at a time
//! with the kernel's editing, echo and Ctrl+C / Ctrl+D handling already done. SIGINT only sets a
//! flag (the handler is installed without `SA_RESTART`, so a blocked `read` or `nanosleep` returns
//! `EINTR` and the flag is looked at).

use crate::shell::{Line, LineSource};
use std::sync::atomic::{AtomicBool, Ordering};

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

pub fn install_sigint_handler() {
    // SAFETY: the handler only stores to an atomic (async-signal-safe).
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
}

/// Whether SIGINT has arrived since the last `take_interrupt`.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// `interrupted()`, and forgets it.
pub fn take_interrupt() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

/// Waits until `fd` (when given) is readable, `timeout` passes, or SIGINT arrives. SIGINT is
/// blocked while the flag is checked and only unblocked inside `pselect`, so one that lands
/// between the check and the wait is not lost (a plain `read` / `nanosleep` would sleep on).
fn wait(fd: Option<libc::c_int>, timeout: Option<libc::timespec>) {
    // SAFETY: all pointers are to live, initialised locals; the mask is restored before returning.
    unsafe {
        let mut block: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut block);
        libc::sigaddset(&mut block, libc::SIGINT);
        libc::sigprocmask(libc::SIG_BLOCK, &block, &mut old);
        if !interrupted() {
            let mut set: libc::fd_set = std::mem::zeroed();
            libc::FD_ZERO(&mut set);
            if let Some(fd) = fd {
                libc::FD_SET(fd, &mut set);
            }
            libc::pselect(
                fd.map_or(0, |fd| fd + 1),
                &mut set,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                timeout.as_ref().map_or(std::ptr::null(), |t| t),
                &old,
            );
        }
        libc::sigprocmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
    }
}

/// Sleeps `secs`, returning early when SIGINT arrives.
pub fn sleep(secs: f64) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
    while !interrupted() {
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return;
        }
        wait(
            None,
            Some(libc::timespec {
                tv_sec: left.as_secs() as libc::time_t,
                tv_nsec: left.subsec_nanos() as _,
            }),
        );
    }
}

/// Lines from file descriptor 0.
#[derive(Default)]
pub struct Stdin {
    buf: Vec<u8>,
}

impl LineSource for Stdin {
    fn next_line(&mut self) -> Line {
        loop {
            if let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=end).collect();
                return Line::Text(String::from_utf8_lossy(&line[..end]).into_owned());
            }
            if interrupted() {
                self.buf.clear();
                return Line::Interrupted;
            }
            wait(Some(0), None);
            if interrupted() {
                continue;
            }
            let mut chunk = [0u8; 4096];
            // SAFETY: `chunk` is a live buffer of the length passed.
            let n = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
            if n > 0 {
                self.buf.extend_from_slice(&chunk[..n as usize]);
            } else if n == 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                // End of input, or the terminal is gone (EIO).
                return Line::Eof;
            }
        }
    }
}
