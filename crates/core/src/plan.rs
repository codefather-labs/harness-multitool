//! Plan mode (ADR 0016): what may run while the agent plans. The engine refuses the rest before
//! hooks and policy, so no approval or remembered answer gets past it.

use std::cell::Cell;

use harness_api::event::HookOutcome;
use harness_api::{BoxFuture, Capability, Hook, HookPoint, ToolCall};

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

/// What `mode-guide` does on a write with no plan made in the session (CON-64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFirst {
    Remind,
    Enforce,
    Off,
}

impl PlanFirst {
    pub fn parse(setting: Option<&str>) -> Self {
        match setting {
            Some("enforce") => PlanFirst::Enforce,
            Some("off") => PlanFirst::Off,
            _ => PlanFirst::Remind,
        }
    }
}

const WHY: &str = "This changes the project and no plan has been made in this session. If this is an app, a feature, a fix or any change that is not trivial, call plan_enter first.";

/// The built-in hook holding to ADR 0021. Whether a task needs a plan stays the model's judgment:
/// this only makes sure the question is asked, once per message, and never touches a read.
pub struct ModeGuide {
    plan_first: PlanFirst,
    planned: Cell<bool>,
    reminded: Cell<bool>,
}

impl ModeGuide {
    pub fn new(plan_first: PlanFirst) -> Self {
        ModeGuide { plan_first, planned: Cell::new(false), reminded: Cell::new(false) }
    }

    fn writes(call: &ToolCall) -> bool {
        match call.name.as_str() {
            "edit" => true,
            "shell" => !call.input["command"].as_str().is_some_and(read_only_command),
            _ => false,
        }
    }
}

impl Hook for ModeGuide {
    fn name(&self) -> &str {
        "mode-guide"
    }

    fn quiet_when_ok(&self) -> bool {
        true
    }

    fn applies(&self, point: &HookPoint<'_>) -> bool {
        self.plan_first != PlanFirst::Off && matches!(point, HookPoint::UserPromptSubmit { .. } | HookPoint::PlanEnter | HookPoint::PreToolUse { .. })
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        let outcome = match point {
            HookPoint::UserPromptSubmit { .. } => {
                self.reminded.set(false);
                HookOutcome::Ok
            }
            HookPoint::PlanEnter => {
                self.planned.set(true);
                HookOutcome::Ok
            }
            HookPoint::PreToolUse { call } if !self.planned.get() && Self::writes(call) => match self.plan_first {
                PlanFirst::Enforce => HookOutcome::Blocked { reason: WHY.to_string() },
                _ if self.reminded.replace(true) => HookOutcome::Ok,
                _ => HookOutcome::Reminder { lines: vec![WHY.to_string()] },
            },
            _ => HookOutcome::Ok,
        };
        Box::pin(std::future::ready(outcome))
    }
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

    fn run(g: &ModeGuide, point: HookPoint<'_>) -> HookOutcome {
        let mut future = g.run(&point);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(o) => o,
            std::task::Poll::Pending => panic!("should be ready"),
        }
    }

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall { id: "c".into(), name: name.into(), input }
    }

    #[test]
    fn the_guide_reminds_once_a_message_and_only_on_writes() {
        let g = ModeGuide::new(PlanFirst::Remind);
        let (edit, ls, journal, rm) = (call("edit", serde_json::json!({})), call("shell", serde_json::json!({"command": "ls"})), call("journal_add", serde_json::json!({})), call("shell", serde_json::json!({"command": "rm a"})));
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &ls }), HookOutcome::Ok);
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &journal }), HookOutcome::Ok);
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &rm }), HookOutcome::Reminder { lines } if lines[0].contains("plan_enter")));
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Ok, "once");
        run(&g, HookPoint::UserPromptSubmit { text: "next" });
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Reminder { .. }), "once a message");
        run(&g, HookPoint::PlanEnter);
        run(&g, HookPoint::UserPromptSubmit { text: "next" });
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Ok, "a plan was made");
    }

    #[test]
    fn enforce_refuses_until_a_plan_and_off_does_nothing() {
        let g = ModeGuide::new(PlanFirst::Enforce);
        let edit = call("edit", serde_json::json!({}));
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Blocked { reason } if reason.contains("plan_enter")));
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Blocked { .. }), "every time");
        run(&g, HookPoint::PlanEnter);
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Ok);
        let off = ModeGuide::new(PlanFirst::parse(Some("off")));
        assert!(!off.applies(&HookPoint::PreToolUse { call: &edit }));
        assert_eq!(PlanFirst::parse(None), PlanFirst::Remind);
    }

    #[test]
    fn writes_are_refused_and_reads_are_not() {
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Memory, Capability::Host("h".into())]).is_none());
        assert!(refusal(&[Capability::WritePath("a".into())]).is_some());
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Command("rm a".into())]).is_some());
        assert!(refusal(&[Capability::Command("send a Telegram message".into())]).is_some());
    }
}
