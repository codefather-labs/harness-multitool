//! The delegate processes the harness started (journal#1021): Claude Code, Codex. A program that
//! ends while a run still holds one would leave it running, since a thread that is still busy is
//! not unwound at exit; the daemon and a window end them all before they exit.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

static CHILDREN: Mutex<BTreeSet<u32>> = Mutex::new(BTreeSet::new());
/// end_all has begun: this program is ending, and no process may start to outlive it.
static ENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A process just started, to be ended with this program if it is still there.
/// Once end_all has begun, it is killed at once instead (journal#1034).
pub fn register(pid: u32) {
    if ENDING.load(std::sync::atomic::Ordering::SeqCst) {
        signal(pid, true);
        return;
    }
    if let Ok(mut set) = CHILDREN.lock() {
        set.insert(pid);
    }
}

/// A process that ended, or that its owner ends itself.
pub fn forget(pid: u32) {
    if let Ok(mut set) = CHILDREN.lock() {
        set.remove(&pid);
    }
}

/// How many are registered.
pub fn count() -> usize {
    CHILDREN.lock().map_or(0, |set| set.len())
}

/// Ends every process registered: asked to stop, then, those still there after `grace`, killed.
/// Answers how many there were.
pub fn end_all(grace: Duration) -> usize {
    ENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    let pids: Vec<u32> = CHILDREN.lock().map(|mut set| std::mem::take(&mut *set).into_iter().collect()).unwrap_or_default();
    for &pid in &pids {
        signal(pid, false);
    }
    let end = Instant::now() + grace;
    while Instant::now() < end && pids.iter().any(|&pid| alive(pid)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    for &pid in pids.iter().filter(|&&pid| alive(pid)) {
        signal(pid, true);
    }
    pids.len()
}

#[cfg(unix)]
fn signal(pid: u32, kill: bool) {
    // SAFETY: kill(2) with a pid this program started; a pid gone since only answers ESRCH.
    unsafe { libc::kill(pid as libc::pid_t, if kill { libc::SIGKILL } else { libc::SIGTERM }) };
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 checks only that the process exists.
    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
        return false;
    }
    // A zombie has ended; its parent, this program, has not reaped it yet.
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| stat.rsplit_once(')').and_then(|(_, rest)| rest.split_whitespace().next()) != Some("Z"))
}

#[cfg(windows)]
fn signal(pid: u32, _kill: bool) {
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// journal#1021: what is registered ends with end_all, by TERM, or by KILL after the grace.
    #[cfg(unix)]
    #[test]
    fn every_registered_process_is_ended() {
        let mut polite = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        let mut stubborn = std::process::Command::new("sh").args(["-c", "trap '' TERM; while :; do sleep 0.05; done"]).spawn().unwrap();
        let mut other = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        register(polite.id());
        register(stubborn.id());
        register(other.id());
        forget(other.id());
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        assert_eq!(end_all(Duration::from_millis(500)), 2);
        assert!(polite.wait().is_ok() && stubborn.wait().is_ok(), "both ended");
        assert!(start.elapsed() < Duration::from_secs(3), "within the grace and a kill: {:?}", start.elapsed());
        assert!(other.try_wait().unwrap().is_none(), "a forgotten process is left alone");
        assert_eq!(count(), 0);
        let _ = other.kill();
        let _ = other.wait();
        // journal#1034: one started after end_all is ended at once, not left to outlive it.
        let mut late = std::process::Command::new("sleep").arg("30").spawn().unwrap();
        register(late.id());
        assert!(late.wait().is_ok_and(|s| !s.success()), "killed at its registration");
        assert_eq!(count(), 0);
    }
}
