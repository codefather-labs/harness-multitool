//! The daemon ends cleanly on SIGTERM and SIGINT (CON-182): systemd's stop sends SIGTERM when its
//! ExecStop could not reach the daemon, and a daemon that died on it left its delegates running
//! (journal#1356). The handler only writes a byte to a pipe, which is async-signal-safe; a thread
//! reads it and does the stop.

#[cfg(unix)]
use std::sync::atomic::{AtomicI32, Ordering};

#[cfg(unix)]
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

#[cfg(unix)]
extern "C" fn on_signal(_: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let byte = 1u8;
        // SAFETY: write(2) on a pipe this module made; async-signal-safe.
        unsafe { libc::write(fd, (&byte as *const u8).cast(), 1) };
    }
}

/// Calls `stop` once, on a thread of its own, at the first SIGTERM or SIGINT of this process.
/// For the daemon's process only: a library must not take a process's signals.
#[cfg(unix)]
pub fn on_stop_signal(stop: impl FnOnce() + Send + 'static) {
    let mut fds = [0 as libc::c_int; 2];
    // SAFETY: pipe(2) fills the two descriptors it is given.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return;
    }
    WRITE_FD.store(fds[1], Ordering::SeqCst);
    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: a zeroed sigaction with a handler that only writes to a pipe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as usize;
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    let read_fd = fds[0];
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        loop {
            // SAFETY: read(2) into a one-byte buffer from the pipe made above.
            let n = unsafe { libc::read(read_fd, byte.as_mut_ptr().cast(), 1) };
            if n == 1 {
                stop();
                return;
            }
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return;
        }
    });
}

#[cfg(not(unix))]
pub fn on_stop_signal(_stop: impl FnOnce() + Send + 'static) {}
