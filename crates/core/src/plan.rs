//! Plan mode (ADR 0016): what may run while the agent plans. The engine refuses the rest before
//! hooks and policy, so no approval or remembered answer gets past it.

use std::cell::Cell;

use harness_api::event::HookOutcome;
use harness_api::{BoxFuture, Capability, Hook, HookPoint, ToolCall};

pub const ENTER_TOOL: &str = "mode__plan_enter";
pub const EXIT_TOOL: &str = "mode__plan_exit";

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
pub fn refusal(capabilities: &[Capability], root: &str) -> Option<String> {
    capabilities.iter().find_map(|c| match c {
        Capability::ReadPath(_) | Capability::Host(_) | Capability::Memory => None,
        Capability::WritePath(path) if in_docs(path, root) => None,
        Capability::WritePath(path) => Some(format!("plan mode: nothing but docs/ may change, so {path} cannot be written now. Make it a work item of the plan: {NEXT}")),
        Capability::Command(command) if read_only_command(command) => None,
        Capability::Command(command) => Some(format!("plan mode: only single read-only commands run, not {command:?}. If the plan needs it, make it a work item: {NEXT}")),
    })
}

const NEXT: &str = "journal_add, mindmap_new, flowchart_new, then mode__plan_exit.";

/// The project's documentation, the one thing written while planning: a feature is described
/// before it is built. A path that climbs out does not count.
fn in_docs(path: &str, root: &str) -> bool {
    let relative = path.strip_prefix(root).map_or(path, |p| p.trim_start_matches(['/', '\\']));
    let mut parts = relative.split(['/', '\\']).filter(|p| !p.is_empty() && *p != ".");
    parts.next() == Some("docs") && parts.next().is_some() && !relative.split(['/', '\\']).any(|p| p == "..")
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

const WHY: &str = "This changes the project and no plan has been made in this session. If this is an app, a feature, a fix or any change that is not trivial, call mode__plan_enter first.";

/// The agent behaviour specification (ADR 0021), each step of it said when it is the step at hand:
/// a small model does not carry a long system prompt through a long run (CON-66).
const REMIND_START: &str = "Agent behaviour spec: an app, a feature, a fix or any change that is not trivial starts with mode__plan_enter, before anything is written.";
const REMIND_PLAN: &str = "Agent behaviour spec, planning: first ask the user what only they can decide, with ask_user. Then journal_add each decision, journal_add a todo with its test protocol for each work item, mindmap_new and mindmap_add what is known, flowchart_new and flowchart_add each flow, describe it under docs/ if the project has docs/. Then mode__plan_exit.";
const REMIND_BUILD: &str = "Agent behaviour spec, the plan is approved. Nothing you tried to write while planning exists yet: build what the plan says with edit, old_text empty to create a file; leave the user's data as it is, run it and read the result. Then call mode__debug with on true and debug it.";
const REMIND_DEBUG: &str = "Agent behaviour spec, debug mode: run each test protocol of the plan and compare what you get with what it should show. Fix what is wrong and run it again. Journal the evidence with journal_add: what ran and what it showed. Then call mode__debug with on false.";
const REMIND_DONE: &str = "Agent behaviour spec: a new feature or fix starts again with mode__plan_enter. Work ends with evidence in the journal, not a claim.";

/// Where the session is in the cycle of ADR 0021.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Start,
    Planning { journal: bool, map: bool, chart: bool, sent_back: bool },
    Building,
    Debugging,
    Done,
}

/// The built-in hook holding to ADR 0021. Whether a task needs a plan stays the model's judgment:
/// this says the step at hand, and on a write with no plan makes sure the question is asked, once
/// per message. It never touches a read, and it holds a run going only once per message.
pub struct ModeGuide {
    plan_first: PlanFirst,
    planned: Cell<bool>,
    reminded: Cell<bool>,
    stage: Cell<Stage>,
    pushed: Cell<bool>,
}

impl ModeGuide {
    pub fn new(plan_first: PlanFirst) -> Self {
        ModeGuide { plan_first, planned: Cell::new(false), reminded: Cell::new(false), stage: Cell::new(Stage::Start), pushed: Cell::new(false) }
    }

    fn writes(call: &ToolCall) -> bool {
        match call.name.as_str() {
            "edit" => true,
            "shell" => !call.input["command"].as_str().is_some_and(read_only_command),
            _ => false,
        }
    }

    fn say(line: &str) -> HookOutcome {
        HookOutcome::Reminder { lines: vec![line.to_string()] }
    }

    /// A plan with nothing recorded goes back once; under `enforce`, until it is recorded.
    fn plan_exit(&self) -> HookOutcome {
        let Stage::Planning { journal, map, chart, sent_back } = self.stage.get() else { return HookOutcome::Ok };
        let missing: Vec<&str> = [(journal, "journal_add"), (map, "mindmap_new"), (chart, "flowchart_new")].iter().filter(|(done, _)| !done).map(|(_, name)| *name).collect();
        if missing.is_empty() || (sent_back && self.plan_first != PlanFirst::Enforce) {
            return HookOutcome::Ok;
        }
        self.stage.set(Stage::Planning { journal, map, chart, sent_back: true });
        HookOutcome::Blocked { reason: format!("The plan is what the journal and the maps hold, so the next session finds it, and this one has none of: {}. Record it with them, ask_user if anything is still open, then call mode__plan_exit again.", missing.join(", ")) }
    }

    /// What using this tool counts towards the plan.
    fn note(&self, name: &str) {
        if let Stage::Planning { journal, map, chart, sent_back } = self.stage.get() {
            self.stage.set(Stage::Planning { journal: journal || name == "journal_add", map: map || name == "mindmap_new", chart: chart || name == "flowchart_new", sent_back });
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
        self.plan_first != PlanFirst::Off
            && matches!(
                point,
                HookPoint::UserPromptSubmit { .. } | HookPoint::PlanEnter | HookPoint::PlanExit | HookPoint::StepDebugEnter | HookPoint::StepDebugExit | HookPoint::PreToolUse { .. } | HookPoint::Stop
            )
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        let outcome = match point {
            HookPoint::UserPromptSubmit { .. } => {
                self.reminded.set(false);
                self.pushed.set(false);
                Self::say(match self.stage.get() {
                    Stage::Start => REMIND_START,
                    Stage::Planning { .. } => REMIND_PLAN,
                    Stage::Building => REMIND_BUILD,
                    Stage::Debugging => REMIND_DEBUG,
                    Stage::Done => REMIND_DONE,
                })
            }
            HookPoint::PlanEnter => {
                self.planned.set(true);
                self.stage.set(Stage::Planning { journal: false, map: false, chart: false, sent_back: false });
                Self::say(REMIND_PLAN)
            }
            HookPoint::PlanExit => {
                self.stage.set(Stage::Building);
                Self::say(REMIND_BUILD)
            }
            HookPoint::StepDebugEnter => {
                self.stage.set(Stage::Debugging);
                Self::say(REMIND_DEBUG)
            }
            HookPoint::StepDebugExit => {
                self.stage.set(Stage::Done);
                HookOutcome::Ok
            }
            // Stopping with the cycle unfinished: said once a message, so it cannot loop, and a
            // question to the user is still there for the user after it.
            HookPoint::Stop => match self.stage.get() {
                Stage::Building | Stage::Debugging if !self.pushed.replace(true) => {
                    let line = if self.stage.get() == Stage::Building { REMIND_BUILD } else { REMIND_DEBUG };
                    HookOutcome::Continue { lines: vec![format!("{line} If you are waiting for the user's answer, say so again and stop; otherwise go on.")] }
                }
                _ => HookOutcome::Ok,
            },
            HookPoint::PreToolUse { call } if call.name == EXIT_TOOL => self.plan_exit(),
            HookPoint::PreToolUse { call } if !self.planned.get() && Self::writes(call) => match self.plan_first {
                PlanFirst::Enforce => HookOutcome::Blocked { reason: WHY.to_string() },
                _ if self.reminded.replace(true) => HookOutcome::Ok,
                _ => HookOutcome::Reminder { lines: vec![WHY.to_string()] },
            },
            HookPoint::PreToolUse { call } => {
                self.note(&call.name);
                HookOutcome::Ok
            }
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
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &rm }), HookOutcome::Reminder { lines } if lines[0].contains("mode__plan_enter")));
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
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Blocked { reason } if reason.contains("mode__plan_enter")));
        assert!(matches!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Blocked { .. }), "every time");
        run(&g, HookPoint::PlanEnter);
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &edit }), HookOutcome::Ok);
        let off = ModeGuide::new(PlanFirst::parse(Some("off")));
        assert!(!off.applies(&HookPoint::PreToolUse { call: &edit }));
        assert_eq!(PlanFirst::parse(None), PlanFirst::Remind);
    }

    #[test]
    fn writes_are_refused_and_reads_are_not() {
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Memory, Capability::Host("h".into())], "/p").is_none());
        assert!(refusal(&[Capability::WritePath("a".into())], "/p").is_some());
        assert!(refusal(&[Capability::ReadPath("a".into()), Capability::Command("rm a".into())], "/p").is_some());
        assert!(refusal(&[Capability::Command("send a Telegram message".into())], "/p").is_some());
    }

    #[test]
    fn only_docs_may_be_written_while_planning() {
        for ok in ["docs/a.md", "docs/spec/a.md", "./docs/a.md", "/p/docs/a.md"] {
            assert!(refusal(&[Capability::WritePath(ok.into())], "/p").is_none(), "{ok}");
        }
        for no in ["docs", "docs/", "src/a.rs", "docs/../src/a.rs", "../docs/a.md", "/other/docs/a.md", "/p/src/a.rs", "mydocs/a.md"] {
            assert!(refusal(&[Capability::WritePath(no.into())], "/p").is_some(), "{no}");
        }
    }

    fn line(o: HookOutcome) -> String {
        match o {
            HookOutcome::Reminder { lines } | HookOutcome::Continue { lines } => lines.join(" "),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_guide_says_each_step_of_the_cycle_when_it_is_the_step_at_hand() {
        let g = ModeGuide::new(PlanFirst::Remind);
        assert!(line(run(&g, HookPoint::UserPromptSubmit { text: "x" })).contains("mode__plan_enter"));
        assert_eq!(run(&g, HookPoint::Stop), HookOutcome::Ok, "nothing is due before a plan");
        assert!(line(run(&g, HookPoint::PlanEnter)).contains("ask the user"));
        assert_eq!(run(&g, HookPoint::Stop), HookOutcome::Ok, "waiting for answers is right while planning");
        assert!(line(run(&g, HookPoint::UserPromptSubmit { text: "answers" })).contains("ask the user"));
        assert!(line(run(&g, HookPoint::PlanExit)).contains("mode__debug"));
        assert!(matches!(run(&g, HookPoint::Stop), HookOutcome::Continue { lines } if lines[0].contains("mode__debug")));
        assert_eq!(run(&g, HookPoint::Stop), HookOutcome::Ok, "once a message: it cannot loop");
        run(&g, HookPoint::UserPromptSubmit { text: "go on" });
        assert!(matches!(run(&g, HookPoint::Stop), HookOutcome::Continue { .. }));
        assert!(line(run(&g, HookPoint::StepDebugEnter)).contains("evidence"));
        run(&g, HookPoint::UserPromptSubmit { text: "go on" });
        assert!(matches!(run(&g, HookPoint::Stop), HookOutcome::Continue { lines } if lines[0].contains("on false")));
        assert_eq!(run(&g, HookPoint::StepDebugExit), HookOutcome::Ok);
        assert_eq!(run(&g, HookPoint::Stop), HookOutcome::Ok, "the cycle is over");
        assert!(line(run(&g, HookPoint::UserPromptSubmit { text: "next" })).contains("new feature"));
    }

    #[test]
    fn a_plan_with_nothing_recorded_goes_back_once_and_under_enforce_until_it_is_recorded() {
        let exit = call(EXIT_TOOL, serde_json::json!({"plan": "p"}));
        let g = ModeGuide::new(PlanFirst::Remind);
        run(&g, HookPoint::PlanEnter);
        run(&g, HookPoint::PreToolUse { call: &call("journal_add", serde_json::json!({})) });
        let back = run(&g, HookPoint::PreToolUse { call: &exit });
        assert!(matches!(&back, HookOutcome::Blocked { reason } if reason.contains("mindmap_new") && reason.contains("flowchart_new") && !reason.contains("journal_add")), "{back:?}");
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &exit }), HookOutcome::Ok, "once: a model without those tools is not stuck");
        let g = ModeGuide::new(PlanFirst::Enforce);
        run(&g, HookPoint::PlanEnter);
        for _ in 0..2 {
            assert!(matches!(run(&g, HookPoint::PreToolUse { call: &exit }), HookOutcome::Blocked { .. }));
        }
        for name in ["journal_add", "mindmap_new", "flowchart_new"] {
            run(&g, HookPoint::PreToolUse { call: &call(name, serde_json::json!({})) });
        }
        assert_eq!(run(&g, HookPoint::PreToolUse { call: &exit }), HookOutcome::Ok);
    }
}
