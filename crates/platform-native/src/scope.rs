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

/// The pid of the program that started an agent's scope, from the scope's unit name
/// (`multitool-agent-<adapter>-<pid>-<n>-<hex>.scope`), or None for any other unit (CON-182).
pub fn starter_of(unit: &str) -> Option<u32> {
    let name = unit.strip_prefix("multitool-agent-").or_else(|| unit.strip_prefix("multitool-callback-"))?.strip_suffix(".scope")?;
    let parts: Vec<&str> = name.rsplitn(4, '-').collect();
    // From the end: hex, n, pid, adapter.
    if parts.len() != 4 || parts[0].is_empty() || !parts[0].chars().all(|c| c.is_ascii_hexdigit()) || parts[1].parse::<u64>().is_err() || parts[3].is_empty() {
        return None;
    }
    parts[2].parse().ok()
}

/// The agents' scopes whose starter is gone (CON-182): `alive` tells whether a pid is still a
/// running multitool.
pub fn orphans<'a>(units: &[&'a str], alive: impl Fn(u32) -> bool) -> Vec<&'a str> {
    units.iter().copied().filter(|unit| starter_of(unit).is_some_and(|pid| !alive(pid))).collect()
}

/// Whether `pid` is a running multitool: a daemon, or a window that runs its sessions itself.
pub fn multitool_alive(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| cmdline.split(|b| *b == 0).next().is_some_and(|program| String::from_utf8_lossy(program).ends_with("multitool")))
}

/// Stops the agents' scopes of every multitool that is gone, with all they still run (CON-182): a
/// daemon that died with no clean end leaves none for long. Answers the units stopped.
pub fn reap() -> Vec<String> {
    if !available() {
        return Vec::new();
    }
    let Ok(out) = Command::new("systemctl").args(["--user", "list-units", "--plain", "--no-legend", "--all", "multitool-agent-*", "multitool-callback-*"]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output() else {
        return Vec::new();
    };
    let listed = String::from_utf8_lossy(&out.stdout).into_owned();
    let units: Vec<&str> = listed.lines().filter_map(|line| line.split_whitespace().next()).collect();
    let gone: Vec<String> = orphans(&units, multitool_alive).into_iter().map(str::to_string).collect();
    if gone.is_empty() {
        return gone;
    }
    // Asked to end, then killed after a short grace: a stop would wait out the scope's own timeout
    // (90 s) for a delegate busy in a tool that does not hear SIGTERM.
    let kill = |signal: &str, units: &[String]| {
        let _ = Command::new("systemctl").args(["--user", "kill", "--kill-whom=all", &format!("--signal={signal}")]).args(units).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
    };
    kill("SIGTERM", &gone);
    let active = |units: &[String]| -> Vec<String> {
        units.iter().filter(|unit| Command::new("systemctl").args(["--user", "is-active", "--quiet", unit]).stdin(std::process::Stdio::null()).status().is_ok_and(|s| s.success())).cloned().collect()
    };
    let end = std::time::Instant::now() + REAP_GRACE;
    let mut left = active(&gone);
    while !left.is_empty() && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(200));
        left = active(&left);
    }
    if !left.is_empty() {
        kill("SIGKILL", &left);
    }
    gone
}

/// How long an orphan's processes have to end on SIGTERM before they are killed (CON-182).
const REAP_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

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

/// The command to run a project's callback handler with (CON-183): in a scope of its own named
/// multitool-callback-<name>-<this pid>-…, so a timeout ends all it started and reap (CON-182)
/// ends what a gone daemon left; else the handler itself. Answers the unit's name too.
pub fn callback_command(handler: &str, name: &str) -> (Command, Option<String>) {
    if !available() || !std::path::Path::new(handler).is_file() {
        return (Command::new(handler), None);
    }
    let unit = unit_name(name).replacen("multitool-agent-", "multitool-callback-", 1);
    let mut command = Command::new("systemd-run");
    command.args(["--user", "--scope", "--quiet", "--collect", "--expand-environment=no"]);
    command.arg(format!("--slice={SLICE}"));
    command.arg(format!("--unit={unit}"));
    command.args(["--", handler]);
    (command, Some(format!("{unit}.scope")))
}

/// Ends a scope and all it runs, at once: a callback handler past its time (CON-183).
pub fn kill_unit(unit: &str) {
    let _ = Command::new("systemctl").args(["--user", "kill", "--kill-whom=all", "--signal=SIGKILL", unit]).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
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

    /// CON-182: an agent's scope names the multitool that started it; only the scopes of one that
    /// is gone are orphans.
    #[test]
    fn an_agents_scope_names_its_starter_and_orphans_are_those_of_the_gone() {
        assert_eq!(starter_of("multitool-agent-claude-1872968-1-28550baf.scope"), Some(1872968));
        assert_eq!(starter_of("multitool-agent-codex-42-0-ff.scope"), Some(42));
        assert_eq!(starter_of("multitool-callback-echo-77-3-1a2b.scope"), Some(77), "a callback handler's scope too (CON-183)");
        assert_eq!(starter_of("multitool-agent-claude-1872968-1-28550baf.service"), None);
        assert_eq!(starter_of("multitool-window-12-0-ab.scope"), None, "a window's scope is never reaped");
        assert_eq!(starter_of("multitool-agent-claude-x-1-ab.scope"), None);
        assert_eq!(starter_of("run-p123-i456.scope"), None);
        let units = ["multitool-agent-claude-100-0-a.scope", "multitool-agent-claude-200-1-b.scope", "multitool-window-100-0-c.scope", "other.scope"];
        assert_eq!(orphans(&units, |pid| pid == 200), ["multitool-agent-claude-100-0-a.scope"]);
        assert!(multitool_alive(std::process::id()) == std::env::current_exe().unwrap().to_string_lossy().ends_with("multitool"), "this test binary is no multitool");
    }

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
