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

/// What a slave needs to prove its work.
const SLAVE_CARGO: [&str; 5] = ["build", "test", "check", "clippy", "fmt"];
/// Options of cargo that reach outside the project or run something else.
const SLAVE_CARGO_REFUSED: [&str; 4] = ["--config", "--target-dir", "--manifest-path", "-Z"];

/// What a slave of an orchestra may do without asking its master (CON-103): files inside the folder
/// it works in, reading commands, and building and testing. Unknown means ask: the list fails closed.
pub fn slave_basics(capabilities: &[Capability], root: &str) -> bool {
    !capabilities.is_empty()
        && capabilities.iter().all(|c| match c {
            Capability::Memory => true,
            Capability::ReadPath(path) | Capability::WritePath(path) => in_project(path, root),
            Capability::Command(command) => slave_chain(command, root),
            Capability::Host(_) => false,
        })
}

/// A command's stages, joined by `;`, `&&`, `||` or `|`, with what is quoted reduced to one word.
/// `None` if anything else outside quotes could redirect, substitute or run in the background: it
/// fails closed. `2>&1` is the one redirection let through: it writes nowhere new.
fn stages(command: &str) -> Option<Vec<String>> {
    let command = command.replace("2>&1", "");
    let (mut out, mut stage) = (Vec::new(), String::new());
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' => {
                let mut quoted = String::new();
                loop {
                    let inner = chars.next()?;
                    if inner == c {
                        break;
                    }
                    if c == '"' && matches!(inner, '$' | '`' | '\\') {
                        return None;
                    }
                    quoted.push(inner);
                }
                // A plain word stays what it is; anything else is just an argument.
                let plain = !quoted.is_empty() && quoted.chars().all(|q| q.is_ascii_alphanumeric() || ",./_-".contains(q));
                stage.push_str(if plain { &quoted } else { "quoted" });
            }
            ';' | '|' => {
                if c == '|' && chars.peek() == Some(&'|') {
                    chars.next();
                }
                out.push(std::mem::take(&mut stage));
            }
            '&' => {
                if chars.next() != Some('&') {
                    return None;
                }
                out.push(std::mem::take(&mut stage));
            }
            '<' | '>' | '$' | '`' | '(' | ')' | '\n' | '\r' | '\\' => return None,
            other => stage.push(other),
        }
    }
    out.push(stage);
    let out: Vec<String> = out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    (!out.is_empty()).then_some(out)
}

/// `timeout 60 cargo test` is `cargo test`.
fn without_timeout(stage: &str) -> String {
    let words: Vec<&str> = stage.split_whitespace().collect();
    match words.as_slice() {
        ["timeout", secs, rest @ ..] if secs.trim_end_matches('s').parse::<u32>().is_ok() => rest.join(" "),
        _ => stage.to_string(),
    }
}

/// A stage that only reads, of a slave's chain: CON-53's commands, and a few that only pass text on.
fn reading_stage(stage: &str, root: &str) -> bool {
    let words: Vec<&str> = stage.split_whitespace().collect();
    match words.as_slice() {
        ["cd", dir] => in_project(dir, root),
        ["echo", ..] | ["cut", ..] | ["tr", ..] | ["true"] => true,
        // With a file named it writes to the second one; from a pipe it only passes text on.
        ["uniq", options @ ..] => options.iter().all(|o| o.starts_with('-')),
        ["sort", options @ ..] => !options.iter().any(|o| o.starts_with("-o") || o.starts_with("--output")),
        // Numbers and p: a script of sed could write or run anything.
        ["sed", "-n", script, files @ ..] => script.strip_suffix('p').is_some_and(|lines| !lines.is_empty() && lines.chars().all(|c| c.is_ascii_digit() || c == ',' || c == '$')) && !files.iter().any(|f| f.starts_with('-')),
        _ => read_only_command(stage),
    }
}

/// A slave's chain that only reads (CON-103): what it may run while it plans.
pub fn reading_chain(command: &str, root: &str) -> bool {
    stages(command).is_some_and(|stages| stages.iter().all(|stage| reading_stage(&without_timeout(stage), root)))
}

/// A slave's chain of reading and of building and testing (CON-103).
fn slave_chain(command: &str, root: &str) -> bool {
    stages(command).is_some_and(|stages| {
        stages.iter().all(|stage| {
            let stage = without_timeout(stage);
            reading_stage(&stage, root) || slave_cargo(&stage)
        })
    })
}

fn slave_cargo(command: &str) -> bool {
    let mut words = command.split_whitespace();
    words.next() == Some("cargo")
        && words.next().is_some_and(|sub| SLAVE_CARGO.contains(&sub))
        && !words.any(|w| SLAVE_CARGO_REFUSED.iter().any(|o| w.starts_with(o)))
}

/// Inside this folder: relative, or under it, and never climbing out.
fn in_project(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches(['/', '\\']);
    let inside = !path.starts_with(['/', '\\']) && !path.contains(':') || path == root || path.strip_prefix(root).is_some_and(|rest| rest.starts_with(['/', '\\']));
    inside && !path.split(['/', '\\']).any(|part| part == "..")
}

/// What a slave's plan still lacks of its contract (CON-111): a mind map pointing at its task, and
/// a flowchart with something in it. Only that they exist: whether they are true is its master's to read.
pub fn agent_plan_lacks<'a>(calls: impl Iterator<Item = (&'a str, &'a serde_json::Value)>, task: u64) -> Option<String> {
    let (mut map, mut chart) = (false, false);
    for (name, input) in calls {
        match name.rsplit("__").next().unwrap_or(name) {
            "mindmap_add" => map |= input["refs"].as_array().is_some_and(|refs| refs.iter().any(|r| r.as_u64() == Some(task))),
            "flowchart_add" => chart = true,
            _ => {}
        }
    }
    let mut lacks = Vec::new();
    if !map {
        lacks.push(format!("a mind map of your strategy, the decisions, the work and the proofs you will produce, with mindmap_add nodes whose refs hold journal#{task}"));
    }
    if !chart {
        lacks.push("a flowchart of the logic of what you build, with flowchart_add".to_string());
    }
    (!lacks.is_empty()).then(|| format!("An agent's plan holds its strategy and its logic, and this one has not: {}. Then call this again.", lacks.join("; and ")))
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

/// Tells the agent where the agent workflow specification is, whenever what it knew is new or gone
/// (CON-98): at a session's start, which a resume also is, and after a compaction.
pub struct WorkflowSpec {
    line: String,
}

impl WorkflowSpec {
    pub fn new(path: &str) -> WorkflowSpec {
        WorkflowSpec { line: format!("The agent workflow specification is at {path}. It binds you: read it now, before acting, with no permission needed.") }
    }
}

impl Hook for WorkflowSpec {
    fn name(&self) -> &str {
        "workflow-spec"
    }

    fn quiet_when_ok(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        let outcome = match point {
            HookPoint::SessionStart | HookPoint::SessionResume | HookPoint::PostCompact => HookOutcome::Reminder { lines: vec![self.line.clone()] },
            _ => HookOutcome::Ok,
        };
        Box::pin(std::future::ready(outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CON-103: chains of reading and of cargo pass, and anything else still goes to the master.
    #[test]
    fn a_slaves_chains_of_reading_and_cargo_ask_no_one() {
        let ok = |c: &str| slave_basics(&[Capability::Command(c.to_string())], "/p");
        for good in ["cargo test -p x 2>&1 | tail -5", "cargo clippy --workspace --all-targets", "grep -E \"a|b\" f | head", "sed -n 1,9p f; ls", "sed -n '40,60p' f && echo ---", "cd /p && git status --short", "cd crates && ls", "timeout 580 cargo build --release 2>&1 | tail -3", "git diff --stat; git log --oneline -3", "ls x || echo none", "cat f | sort | cut -c1-40", "cargo test 2>&1 | grep -E \"a|^b\" | sort | uniq -c | head -40"] {
            assert!(ok(good), "{good}");
        }
        for bad in ["cargo test > f", "ls; rm f", "cat $(x)", "cat `x`", "grep \"$HOME\" f", "sed -i s/a/b/ f", "sed -n 1w/tmp/x f", "sed -n '1,2p;w x' f", "sleep 1 &", "cargo test --target-dir x | tail", "python3 x.py", "echo a > f", "cat < f", "cd /etc && ls", "cd .. && ls", "sort -o f g", "uniq a b", "git checkout -- f", "ls\nrm f", "'unclosed"] {
            assert!(!ok(bad), "{bad}");
        }
        assert!(reading_chain("grep x f | head; git status", "/p"));
        assert!(!reading_chain("cargo test | tail", "/p"), "while planning, nothing is built");
        assert!(!read_only_command("ls | head"), "a user's own plan mode stays as it was (CON-53)");
    }

    /// CON-111.
    #[test]
    fn a_slaves_plan_needs_a_map_at_its_task_and_a_chart_with_blocks() {
        let refs = serde_json::json!({"refs": [575, 7]});
        let other = serde_json::json!({"refs": [8]});
        let none = serde_json::json!({});
        let lacks = |calls: &[(&'static str, &serde_json::Value)]| agent_plan_lacks(calls.iter().copied(), 7);
        let both = lacks(&[]).unwrap();
        assert!(both.contains("journal#7") && both.contains("flowchart_add"), "{both}");
        let chart = lacks(&[("mcp__harness__mindmap_add", &refs)]).unwrap();
        assert!(!chart.contains("mindmap_add") && chart.contains("flowchart_add"), "{chart}");
        assert!(lacks(&[("mindmap_add", &other), ("flowchart_add", &none)]).unwrap().contains("journal#7"), "a map at something else");
        assert!(lacks(&[("mindmap_new", &none), ("flowchart_new", &none)]).is_some(), "an empty map and chart");
        assert!(lacks(&[("mindmap_add", &refs), ("mcp__harness__flowchart_add", &none)]).is_none());
    }

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

    #[test]
    fn the_workflow_spec_is_named_at_a_start_and_after_a_compaction_only() {
        let hook = WorkflowSpec::new("/data/spec/agent-workflow.md");
        let says = |point: HookPoint<'_>| {
            let mut future = hook.run(&point);
            let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
            match future.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(outcome) => outcome,
                std::task::Poll::Pending => panic!("it has nothing to wait for"),
            }
        };
        for point in [HookPoint::SessionStart, HookPoint::SessionResume, HookPoint::PostCompact] {
            assert!(matches!(says(point), HookOutcome::Reminder { lines } if lines[0].contains("/data/spec/agent-workflow.md")));
        }
        assert_eq!(says(HookPoint::Stop), HookOutcome::Ok);
        assert_eq!(says(HookPoint::UserPromptSubmit { text: "x" }), HookOutcome::Ok);
    }

    #[test]
    fn a_slave_needs_no_word_for_its_folder_its_reading_and_its_tests_only() {
        let root = "/work/app";
        let ok = |c: Capability| slave_basics(&[c], root);
        for path in ["src/a.rs", "/work/app/src/a.rs", "/work/app", "docs/x.md"] {
            assert!(ok(Capability::WritePath(path.into())), "{path}");
            assert!(ok(Capability::ReadPath(path.into())), "{path}");
        }
        for path in ["/work/application/a", "/etc/passwd", "../other", "src/../../other", "/work/app/../secret", "C:\\x"] {
            assert!(!ok(Capability::WritePath(path.into())), "{path}");
        }
        for command in ["cargo test -p x", "cargo build --release", "cargo clippy", "ls docs", "git status", "git log -3"] {
            assert!(ok(Capability::Command(command.into())), "{command}");
        }
        for command in ["cargo install x", "cargo test; rm -rf /", "cargo test && curl x", "cargo build --target-dir /tmp", "cargo test --config x", "rm a", "git push", "git commit -m x", "python3 x.py", "cargo"] {
            assert!(!ok(Capability::Command(command.into())), "{command}");
        }
        assert!(!ok(Capability::Host("example.com".into())), "the network asks");
        assert!(slave_basics(&[Capability::Memory], root));
        assert!(!slave_basics(&[Capability::Memory, Capability::Command("rm a".into())], root), "all of them, or ask");
    }
}
