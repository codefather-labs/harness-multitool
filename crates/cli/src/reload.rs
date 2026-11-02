//! The hot reload (ADR 0015, CON-50): rebuild, wait for a safe point, hand the view over, swap
//! the binary in the same terminal. The next process replays each session's log (INV-5), so the
//! hand-off only holds what the logs do not.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TabHandoff {
    pub title: String,
    pub session: Option<String>,
    pub window: Option<String>,
    pub url: Option<String>,
    pub scroll: usize,
    pub draft: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Handoff {
    pub active: usize,
    pub theme: String,
    pub tabs: Vec<TabHandoff>,
    /// The tab whose agent asked for this: it is told when it is done.
    #[serde(default)]
    pub tell: Option<usize>,
}

pub fn write_handoff(data_dir: &Path, handoff: &Handoff) -> Result<PathBuf> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join("handoff.json");
    std::fs::write(&path, serde_json::to_string(handoff)?).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Reads the hand-off and deletes it: it is good for one start.
pub fn take_handoff(path: &Path) -> Result<Handoff> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let _ = std::fs::remove_file(path);
    Ok(serde_json::from_str(&text)?)
}

/// Builds in the background. `MULTITOOL_RELOAD_BUILD` replaces the build command.
pub fn start_build() -> Receiver<Result<(), String>> {
    let (done, result) = channel();
    std::thread::spawn(move || {
        let _ = done.send(build());
    });
    result
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

fn build() -> Result<(), String> {
    let mut command = match std::env::var("MULTITOOL_RELOAD_BUILD") {
        Ok(custom) if !custom.is_empty() => shell(&custom),
        _ => {
            // The workspace this binary was built from, and only if this is that build's output.
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
            let expected = workspace.join("target").join(profile).join(format!("multitool{}", std::env::consts::EXE_SUFFIX));
            let same = |a: &Path, b: &Path| a.canonicalize().ok().is_some_and(|x| b.canonicalize().ok().is_some_and(|y| x == y));
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            if !same(&exe, &expected) {
                return Err(format!("this binary ({}) is not the output of a local build, so there is nothing to rebuild", exe.display()));
            }
            let mut c = Command::new("cargo");
            c.args(["build", "-q", "-p", "multitool"]).current_dir(workspace);
            if !cfg!(debug_assertions) {
                c.arg("--release");
            }
            // The new binary has what this one has.
            if cfg!(feature = "claude-code") {
                c.args(["--features", "claude-code"]);
            }
            c
        }
    };
    let output = command.stdin(std::process::Stdio::null()).output().map_err(|e| format!("starting the build: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
    Err(format!("the build failed:\n{}", tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
}

/// Replaces this process with the new binary, in the same terminal. On Windows, where a process
/// cannot replace itself, it runs the new one in the same console and passes its exit code on.
pub fn swap(handoff: &Path, theme: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command.args(["preview", "welcome", "--theme", theme, "--handoff"]).arg(handoff);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("swapping the binary")
    }
    #[cfg(not(unix))]
    {
        let status = command.status().context("starting the new binary")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handoff_round_trips_once() {
        let dir = std::env::temp_dir().join(format!("mt-handoff-{}", std::process::id()));
        let h = Handoff {
            tell: None,
            active: 1,
            theme: "dark".into(),
            tabs: vec![TabHandoff { title: "a".into(), session: Some("s".into()), window: Some("w".into()), url: Some("u".into()), scroll: 3, draft: "half a thought".into() }],
        };
        let path = write_handoff(&dir, &h).unwrap();
        assert_eq!(take_handoff(&path).unwrap(), h);
        assert!(take_handoff(&path).is_err(), "good for one start");
        let _ = std::fs::remove_dir_all(dir);
    }
}
