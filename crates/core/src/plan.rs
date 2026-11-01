//! Plan mode (ADR 0016): what may run while the agent plans. The engine refuses the rest before
//! hooks and policy, so no approval or remembered answer gets past it.

use harness_api::Capability;

pub const ENTER_TOOL: &str = "plan_enter";
pub const EXIT_TOOL: &str = "plan_exit";

/// Commands that only read, when they stand alone with no operators.
const READ_ONLY: [&str; 11] = ["ls", "cat", "head", "tail", "wc", "grep", "rg", "pwd", "stat", "du", "df"];
const READ_ONLY_GIT: [&str; 5] = ["status", "log", "diff", "show", "blame"];
/// Options of those commands that write a file or run a program.
const REFUSED_OPTIONS: [&str; 3] = ["--output", "--pre", "--ext-diff"];
/// Anything that could chain, redirect or substitute.
const SHELL_OPERATORS: [char; 11] = [';', '&', '|', '<', '>', '$', '`', '\n', '\r', '(', ')'];

/// One known read-only command. Unknown means no: the list fails closed.
pub fn read_only_command(command: &str) -> bool {
    if command.contains(SHELL_OPERATORS) {
        return false;
    }
    let mut words = command.split_whitespace();
    let known = match words.next() {
        Some("git") => words.next().is_some_and(|sub| READ_ONLY_GIT.contains(&sub)),
        Some(program) => READ_ONLY.contains(&program),
        None => false,
    };
    known && !command.split_whitespace().any(|w| REFUSED_OPTIONS.iter().any(|o| w.starts_with(o)))
}

/// Why plan mode refuses this, if it does.
pub fn refusal(capabilities: &[Capability]) -> Option<String> {
    capabilities.iter().find_map(|c| match c {
        Capability::ReadPath(_) | Capability::Host(_) | Capability::Memory => None,
        Capability::WritePath(path) => Some(format!("plan mode: nothing in the project may change, so {path} cannot be written")),
        Capability::Command(command) if read_only_command(command) => None,
        Capability::Command(command) => Some(format!("plan mode: only single read-only commands run, not {command:?}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_known_reading_commands_pass() {
        for ok in ["ls -la src", "cat a.rs", "git log -5", "git diff HEAD~1", "grep -n foo a.rs", "rg foo", "wc -l a"] {
            assert!(read_only_command(ok), "{ok}");
        }
        for no in [
            "", "rm -rf x", "git push", "git commit -m x", "git -c core.pager=x log", "git branch -D x", "cat a; rm b", "cat a && rm b", "cat a | sh", "cat a > b", "cat $(rm b)", "cat `rm b`", "ls\nrm b",
            "git log --output=a", "git diff --ext-diff", "rg --pre ./x foo", "find . -delete", "sed -i s/a/b/ c", "python3 -c x", "cargo test",
        ] {
            assert!(!read_only_command(no), "{no}");
        }
    }

    #[test]
    fn writes_are_refused_and_reads_are_not() {
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Memory, Capability::Host("h".into())]).is_none());
        assert!(refusal(&[Capability::WritePath("a".into())]).is_some());
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Command("rm a".into())]).is_some());
        assert!(refusal(&[Capability::Command("send a Telegram message".into())]).is_some());
    }
}
