//! A delegate program in a systemd user scope of its own (CON-166): its MCP servers and what it
//! runs are counted apart from the daemon, so the service's memory is the daemon's alone and each
//! agent's can be seen by itself (`systemd-cgtop`, `multitool-agents.slice`).
//!
//! `systemd-run --scope` runs the program in the very process that was spawned: its pid, its
//! process group and its pipes are the caller's (journal#1225). Arguments go as they are, with
//! `--expand-environment=no`, which systemd-run would otherwise expand (`$HOME`, `$$`). Where there
//! is no user manager to ask, the program starts as it always did.

use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// The slice every agent's scope goes under.
pub const SLICE: &str = "multitool-agents.slice";

/// What the user manager needs to be reached, carried into a cleared environment.
const BUS: [&str; 2] = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"];

/// Whether scopes can be made here: checked once, with the very flags used after.
fn available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        cfg!(target_os = "linux")
            && std::env::var_os("MULTITOOL_AGENT_SCOPES").is_none_or(|v| v != "0")
            && Command::new("systemd-run")
                .args(["--user", "--scope", "--quiet", "--collect", "--expand-environment=no", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    })
}

/// A unit name no other scope has: the adapter, this process and a counter.
fn unit_name(adapter: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let safe: String = adapter.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    format!("multitool-agent-{safe}-{}-{}-{nanos:x}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

/// The arguments that put `program` in a scope of its own, before its own arguments.
fn wrapper_args(program: &str, adapter: &str) -> Vec<String> {
    let mut args: Vec<String> = ["--user", "--scope", "--quiet", "--collect", "--expand-environment=no"].map(String::from).to_vec();
    args.push(format!("--slice={SLICE}"));
    args.push(format!("--unit={}", unit_name(adapter)));
    args.push("--".to_string());
    args.push(program.to_string());
    args
}

/// The command to start a delegate program with; its own arguments follow. In a scope of its own
/// when the user manager can make one, else the program itself.
pub fn agent_command(program: &str, adapter: &str) -> Command {
    // A program that is not there fails to start as it always did, saying so, not as a scope
    // whose program then cannot be run.
    if !available() || !found(program) {
        return Command::new(program);
    }
    let mut command = Command::new("systemd-run");
    command.args(wrapper_args(program, adapter));
    command
}

/// Whether `program` can be run: a path that is a file, or a name found on PATH.
fn found(program: &str) -> bool {
    let path = std::path::Path::new(program);
    if program.contains(std::path::MAIN_SEPARATOR) {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| std::env::split_paths(&dirs).any(|dir| dir.join(program).is_file()))
}

/// The command to start a terminal window with (CON-167): in a scope of its own, so that the
/// daemon that opened it, and its restarts, do not take it down; else the program itself.
pub fn window_command(program: &str) -> Command {
    if !available() || !found(program) {
        return Command::new(program);
    }
    let mut command = Command::new("systemd-run");
    command.args(["--user", "--scope", "--quiet", "--collect", "--expand-environment=no"]);
    command.arg(format!("--unit={}", unit_name("window").replacen("multitool-agent-window", "multitool-window", 1)));
    command.args(["--", program]);
    command
}

/// After an `env_clear`, what systemd-run needs to reach the user manager, from this process,
/// unless the given environment has it already.
pub fn keep_bus(command: &mut Command, given: &std::collections::BTreeMap<String, String>) {
    if command.get_program() != "systemd-run" {
        return;
    }
    for name in BUS {
        if !given.contains_key(name)
            && let Some(value) = std::env::var_os(name)
        {
            command.env(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program follows the wrapper's flags, after `--`, with the expansion off and a unit of
    /// its own each time.
    #[test]
    fn the_wrapper_keeps_the_program_and_its_arguments_as_they_are() {
        let args = wrapper_args("/usr/bin/claude", "claude-code");
        assert_eq!(&args[..5], ["--user", "--scope", "--quiet", "--collect", "--expand-environment=no"]);
        assert_eq!(args[5], "--slice=multitool-agents.slice");
        assert!(args[6].starts_with("--unit=multitool-agent-claude_code-"), "{}", args[6]);
        assert_eq!(&args[7..], ["--", "/usr/bin/claude"]);
        assert_ne!(wrapper_args("/usr/bin/claude", "claude-code")[6], args[6], "each its own unit");
    }

    /// A program that is not there is started itself, so its spawn fails as before.
    #[test]
    fn a_missing_program_is_not_wrapped() {
        assert_eq!(agent_command("/no/such/claude", "x").get_program(), "/no/such/claude");
        assert_eq!(agent_command("no-such-program-anywhere", "x").get_program(), "no-such-program-anywhere");
    }

    /// Where scopes are off the program is started itself.
    #[test]
    fn off_the_program_starts_itself() {
        if !available() {
            assert_eq!(agent_command("/bin/true", "x").get_program(), "/bin/true");
        } else {
            let command = agent_command("/bin/true", "x");
            assert_eq!(command.get_program(), "systemd-run");
            // And it runs, in the same process, with its argument kept.
            let out = agent_command("sh", "test").args(["-c", "echo $$ '$HOME'"]).output().unwrap();
            let said = String::from_utf8_lossy(&out.stdout);
            assert!(said.trim().ends_with("$HOME"), "{said}");
        }
    }
}
