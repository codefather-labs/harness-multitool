//! The daemon as a user service, as install.sh installs it (CON-162). With one installed for a data
//! directory, every start of that directory's daemon goes through it, and a restart is one step of
//! the service manager: a window that finds no daemon never starts one beside it (journal#1181).
//!
//! The service serves the default data directory only, the one HOME alone gives: a test or a
//! temporary home with a data directory of its own never touches the user's service.
//!
//! `multitool daemon stop` stops the daemon through its own socket, never through the service: the
//! service's ExecStop is that very command.

use std::path::{Path, PathBuf};

/// The systemd user unit's name.
pub const UNIT: &str = "multitool-daemon";
/// The macOS LaunchAgent's label.
pub const LAUNCH_AGENT: &str = "dev.multitool.daemon";
/// The Windows Scheduled Task's name, as install.ps1 registers it.
pub const TASK: &str = "multitool-daemon";

/// Which service manager runs the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Systemd,
    Launchd,
    /// A Scheduled Task run at the user's logon (Windows).
    Task,
}

/// The data directory the service's daemon serves: the default one, from HOME alone.
fn served(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") { home.join("Library/Application Support/harness-multitool") } else { home.join(".local/share/harness-multitool") }
}

/// The unit file install.sh writes.
fn unit_file(home: &Path) -> PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map_or_else(|| home.join(".config"), PathBuf::from);
    config.join("systemd/user").join(format!("{UNIT}.service"))
}

fn plist(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents").join(format!("{LAUNCH_AGENT}.plist"))
}

/// The service that runs `data_dir`'s daemon, if one is installed.
pub fn installed_for(data_dir: &Path) -> Option<Kind> {
    // Windows: the data directory is the roaming AppData's, as the dirs crate gives it.
    if cfg!(windows) {
        let served = PathBuf::from(std::env::var_os("APPDATA")?).join("harness-multitool");
        let same = data_dir.canonicalize().ok().zip(served.canonicalize().ok()).is_some_and(|(a, b)| a == b);
        return (same && run("schtasks", &["/Query", "/TN", TASK]).is_ok()).then_some(Kind::Task);
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let ours = |a: &Path, b: &Path| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b);
    if !ours(data_dir, &served(&home)) {
        return None;
    }
    if cfg!(target_os = "linux") && unit_file(&home).exists() {
        return Some(Kind::Systemd);
    }
    if cfg!(target_os = "macos") && plist(&home).exists() {
        return Some(Kind::Launchd);
    }
    None
}

fn run(program: &str, args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map_err(|e| format!("{program}: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("`{program} {}` ended with {status}", args.join(" "))) }
}

/// launchd's domain for this user: the owner of HOME.
#[cfg(unix)]
fn gui_domain() -> Result<String, String> {
    use std::os::unix::fs::MetadataExt;
    let home = std::env::var_os("HOME").ok_or("no HOME")?;
    let uid = std::fs::metadata(home).map_err(|e| e.to_string())?.uid();
    Ok(format!("gui/{uid}/{LAUNCH_AGENT}"))
}
#[cfg(not(unix))]
fn gui_domain() -> Result<String, String> {
    Err("launchd is macOS's".to_string())
}

/// Starts the service; one already running is left as it is.
pub fn start(kind: Kind) -> Result<(), String> {
    match kind {
        Kind::Systemd => run("systemctl", &["--user", "start", UNIT]),
        Kind::Launchd => run("launchctl", &["kickstart", &gui_domain()?]),
        Kind::Task => run("schtasks", &["/Run", "/TN", TASK]),
    }
}

/// Restarts the service in one step: its stop ends the daemon cleanly (ExecStop), and the new one
/// follows with no gap for another starter.
pub fn restart(kind: Kind) -> Result<(), String> {
    match kind {
        Kind::Systemd => run("systemctl", &["--user", "restart", UNIT]),
        Kind::Launchd => run("launchctl", &["kickstart", "-k", &gui_domain()?]),
        // The task's end stops its process; the daemon's own stop is not run there.
        Kind::Task => {
            let _ = run("schtasks", &["/End", "/TN", TASK]);
            run("schtasks", &["/Run", "/TN", TASK])
        }
    }
}

/// Whether the service has its daemon running now.
pub fn active(kind: Kind) -> bool {
    match kind {
        Kind::Systemd => run("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok(),
        Kind::Launchd => gui_domain().is_ok_and(|domain| run("launchctl", &["print", &domain]).is_ok()),
        Kind::Task => std::process::Command::new("schtasks")
            .args(["/Query", "/TN", TASK, "/FO", "CSV", "/NH"])
            .stdin(std::process::Stdio::null())
            .output()
            .is_ok_and(|out| out.status.success() && String::from_utf8_lossy(&out.stdout).contains("\"Running\"")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A data directory other than the default is never the service's, whatever is installed.
    #[test]
    fn only_the_default_data_directory_is_the_services() {
        let temp = std::env::temp_dir().join(format!("mt-service-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        assert_eq!(installed_for(&temp), None);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
