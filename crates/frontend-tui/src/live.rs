//! The running session in the view: a pure mapping from log entries to transcript items
//! (IF-11, INV-5). What is on screen is a fold over the log, so a resumed session draws itself
//! the same way, and `multitool screen --scenario` shows any point of a real run.

use std::collections::HashMap;

use harness_api::event::{DecidedBy, HookOutcome as LogOutcome};
use harness_api::{Capability, Event, LogEntry, StopReason, ToolCall as LogCall};
use serde_json::Value;

use crate::model::{Diff, DiffLine, HookEvent, HookOutcome, HookRun, Item, PermissionView, Role, SessionView, Tone, ToolBody, ToolCall, ToolIcon, ToolState};

/// Tool output lines shown before `+N more lines`.
const OUTPUT_LINES: usize = 8;

/// Turns a log time into the stamp a card shows.
pub type TimeFormat = fn(u64) -> String;

/// What the mapping remembers between entries: where each tool call's card is.
pub struct Feed {
    time: TimeFormat,
    calls: HashMap<String, CallPlace>,
    cost_micro_usd: u64,
    context_window: u64,
}

struct CallPlace {
    item: usize,
    attached: usize,
    started_ms: u64,
    input: Value,
    name: String,
}

impl Feed {
    pub fn new(time: TimeFormat, context_window: u64) -> Self {
        Feed { time, calls: HashMap::new(), cost_micro_usd: 0, context_window }
    }

    pub fn apply(&mut self, v: &mut SessionView, entry: &LogEntry) {
        let time = (self.time)(entry.at_ms);
        match &entry.event {
            Event::UserMessage { text } => {
                v.transcript.push(Item::Turn { role: Role::User, time, lines: text.lines().map(str::to_string).collect(), attached: vec![] });
            }
            Event::TaskState { .. } | Event::OpaqueBlock { .. } | Event::ModelToolCall { .. } => {}
            Event::TurnStarted { provider, model, .. } => {
                v.generating = true;
                set_status(v, "ADAPTER", provider);
                set_status(v, "MODEL", model);
            }
            Event::TextDelta { text } => append_text(v, time, text),
            Event::TurnFinished { stop, usage, cost_micro_usd, .. } => {
                self.cost_micro_usd += cost_micro_usd.unwrap_or(0);
                // How full the context is: the last turn's whole prompt and its answer.
                let used = usage.input + usage.cache_read + usage.cache_write + usage.output;
                set_status(v, "TOKENS", &format!("{} / {}", short(used), short(self.context_window)));
                set_status(v, "COST", &format!("${:.2}", self.cost_micro_usd as f64 / 1e6));
                if !matches!(stop, StopReason::ToolUse) {
                    v.generating = false;
                }
            }
            Event::ToolRequested { call, .. } => {
                let card = ToolCall {
                    name: call.name.clone(),
                    args: summary(&call.input),
                    args_strong: None,
                    state: ToolState::Running,
                    outcome: String::new(),
                    outcome_tone: Tone::Neutral,
                    diffstat: None,
                    duration: String::new(),
                    expanded: false,
                    body: ToolBody::None,
                    icon: if call.name == "shell" { ToolIcon::Shell } else { ToolIcon::None },
                };
                let (item, attached) = attach(v, time, Item::Tool(card));
                self.calls.insert(call.id.clone(), CallPlace { item, attached, started_ms: entry.at_ms, input: call.input.clone(), name: call.name.clone() });
            }
            Event::ToolDecision { id, allowed: false, by, reason } => {
                if let Some(card) = self.card(v, id) {
                    card.outcome = match by {
                        DecidedBy::User => "denied".to_string(),
                        DecidedBy::Policy => "denied by policy".to_string(),
                        DecidedBy::Hook => "blocked by a hook".to_string(),
                    };
                    card.outcome_tone = Tone::Error;
                    card.state = ToolState::Failed;
                    if let Some(reason) = reason {
                        card.body = ToolBody::Note(reason.clone());
                    }
                }
            }
            Event::ToolDecision { .. } => {}
            Event::ToolResult { id, output, is_error } => {
                let started = self.calls.get(id).map(|c| c.started_ms);
                let diff = self.calls.get(id).filter(|c| c.name == "edit" && !*is_error).and_then(|c| edit_diff(&c.input));
                if let Some(card) = self.card(v, id) {
                    let denied = card.state == ToolState::Failed;
                    card.state = if *is_error { ToolState::Failed } else { ToolState::Done };
                    if let Some(started) = started {
                        card.duration = duration(entry.at_ms.saturating_sub(started));
                    }
                    if !denied {
                        card.outcome = if *is_error { "failed".into() } else { "completed".into() };
                        card.outcome_tone = if *is_error { Tone::Error } else { Tone::Success };
                        card.body = output_body(output);
                        if let Some((diff, added, removed)) = diff {
                            card.outcome = "applied".into();
                            card.diffstat = Some((added, removed));
                            card.body = ToolBody::Diff(diff);
                        }
                    }
                }
            }
            Event::HookRan { point, hook, outcome, .. } => {
                let run = HookRun { script: hook.clone(), outcome: hook_outcome(outcome), duration: String::new() };
                let name = format!("{point:?}");
                match v.transcript.last_mut() {
                    Some(Item::Hook(h)) if h.event == name => h.runs.push(run),
                    _ => v.transcript.push(Item::Hook(HookEvent { event: name, subject: None, time, runs: vec![run] })),
                }
            }
            Event::Verification { pass, evidence } => {
                let label = if *pass { "VERIFICATION PASSED" } else { "VERIFICATION FAILED" };
                v.transcript.push(Item::Tips { label: label.into(), lines: evidence.lines().map(str::to_string).collect() });
            }
            Event::Error { source, message } => {
                v.generating = false;
                v.transcript.push(Item::Tips { label: format!("ERROR · {source}"), lines: message.lines().map(str::to_string).collect() });
            }
        }
    }

    /// The run has ended, however it ended: nothing is generating any more.
    pub fn run_ended(&mut self, v: &mut SessionView) {
        v.generating = false;
    }

    fn card<'a>(&self, v: &'a mut SessionView, id: &str) -> Option<&'a mut ToolCall> {
        let place = self.calls.get(id)?;
        match v.transcript.get_mut(place.item)? {
            Item::Turn { attached, .. } => match attached.get_mut(place.attached)? {
                Item::Tool(card) => Some(card),
                _ => None,
            },
            _ => None,
        }
    }
}

/// The permission prompt for a call `Policy` asked about (permission-*).
pub fn permission(tab: u64, call: &LogCall, capabilities: &[Capability], workdir: &str) -> PermissionView {
    let needs: Vec<String> = capabilities
        .iter()
        .map(|c| match c {
            Capability::ReadPath(p) => format!("read {p}"),
            Capability::WritePath(p) => format!("write {p}"),
            Capability::Command(cmd) => format!("run {cmd}"),
            Capability::Host(h) => format!("connect to {h}"),
            Capability::Memory => "write to memory".to_string(),
        })
        .collect();
    PermissionView {
        tool: call.name.clone(),
        summary: format!("The agent wants to use the tool {}.", call.name),
        command: summary(&call.input),
        workdir: workdir.to_string(),
        scope: if needs.is_empty() { "nothing declared".into() } else { needs.join(" · ") },
        policy: "approval required".into(),
        choice: 0,
        tab: Some(tab),
    }
}

fn append_text(v: &mut SessionView, time: String, text: &str) {
    // Text after a tool call starts a new message under it, in reading order.
    let open = matches!(v.transcript.last(), Some(Item::Turn { role: Role::Assistant, attached, .. }) if attached.is_empty());
    if !open {
        v.transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec![String::new()], attached: vec![] });
    }
    let Some(Item::Turn { lines, .. }) = v.transcript.last_mut() else { return };
    let mut parts = text.split('\n');
    if let Some(first) = parts.next() {
        match lines.last_mut() {
            Some(last) => last.push_str(first),
            None => lines.push(first.to_string()),
        }
    }
    lines.extend(parts.map(str::to_string));
}

/// Hangs an item on the newest assistant turn, opening one if the agent went straight to a tool.
fn attach(v: &mut SessionView, time: String, item: Item) -> (usize, usize) {
    if !matches!(v.transcript.last(), Some(Item::Turn { role: Role::Assistant, .. })) {
        v.transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec![], attached: vec![] });
    }
    let index = v.transcript.len() - 1;
    let Some(Item::Turn { attached, .. }) = v.transcript.last_mut() else { unreachable!("just ensured") };
    attached.push(item);
    (index, attached.len() - 1)
}

fn set_status(v: &mut SessionView, key: &str, value: &str) {
    match v.status.fields.iter_mut().find(|(k, _)| k == key) {
        Some((_, old)) => *old = value.to_string(),
        None => v.status.fields.push((key.to_string(), value.to_string())),
    }
}

fn output_body(output: &str) -> ToolBody {
    let all: Vec<&str> = output.lines().collect();
    if all.is_empty() {
        return ToolBody::None;
    }
    let shown = all.iter().take(OUTPUT_LINES).map(|l| l.to_string()).collect();
    ToolBody::Output { label: None, lines: shown, numbered: false, more: all.len().saturating_sub(OUTPUT_LINES) }
}

fn hook_outcome(outcome: &LogOutcome) -> HookOutcome {
    match outcome {
        LogOutcome::Ok => HookOutcome::Ok,
        LogOutcome::Context { lines } => HookOutcome::Context(lines.clone()),
        LogOutcome::Blocked { reason } => HookOutcome::Blocked(reason.clone()),
        LogOutcome::Failed { message } => HookOutcome::Failed { code: 1, stderr: message.lines().map(str::to_string).collect() },
    }
}

/// An `edit` call as a diff: what it replaced with what.
fn edit_diff(input: &Value) -> Option<(Diff, u32, u32)> {
    let (old, new) = (input["old_text"].as_str()?, input["new_text"].as_str()?);
    let mut lines: Vec<DiffLine> = old.lines().take(OUTPUT_LINES).map(|l| DiffLine::Removed(l.to_string())).collect();
    lines.extend(new.lines().take(OUTPUT_LINES).map(|l| DiffLine::Added(l.to_string())));
    let (added, removed) = (new.lines().count() as u32, old.lines().count() as u32);
    Some((Diff { hunk: format!("@@ -{removed} +{added} @@"), context: input["path"].as_str().unwrap_or_default().to_string(), lines }, added, removed))
}

/// A call's arguments on one line: what the call is about, or else compact JSON.
fn summary(input: &Value) -> String {
    const MAIN_FIELDS: [&str; 5] = ["command", "path", "url", "text", "query"];
    let main = MAIN_FIELDS.iter().find_map(|f| input[*f].as_str()).map(str::to_string);
    let text = match (main, input) {
        (Some(main), _) => main,
        (None, Value::Object(fields)) if fields.len() == 1 => match fields.values().next() {
            Some(Value::String(s)) => s.clone(),
            _ => input.to_string(),
        },
        (None, Value::Null) => String::new(),
        _ => input.to_string(),
    };
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > 100 { format!("{}…", line.chars().take(99).collect::<String>()) } else { line.to_string() }
}

fn short(n: u64) -> String {
    match n {
        1_000_000.. if n % 1_000_000 == 0 => format!("{}M", n / 1_000_000),
        1000.. => format!("{}k", n / 1000),
        _ => n.to_string(),
    }
}

fn duration(ms: u64) -> String {
    if ms >= 1000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{ms}ms") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_api::Usage;
    use harness_api::event::HookPointKind;
    use serde_json::json;

    fn entry(seq: u64, event: Event) -> LogEntry {
        LogEntry { seq, at_ms: seq * 10, event }
    }

    fn feed() -> Feed {
        Feed::new(|ms| format!("t{ms}"), 200_000)
    }

    fn call(id: &str) -> Event {
        Event::ToolRequested { call: LogCall { id: id.into(), name: "shell".into(), input: json!({"command": "cargo test"}) }, capabilities: vec![] }
    }

    fn first_tool(v: &SessionView, turn: usize) -> &ToolCall {
        let Item::Turn { attached, .. } = &v.transcript[turn] else { panic!("not a turn") };
        let Item::Tool(card) = &attached[0] else { panic!("not a tool") };
        card
    }

    #[test]
    fn deltas_grow_one_message_and_keep_line_breaks() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &entry(0, Event::UserMessage { text: "hi".into() }));
        for (i, t) in ["Hel", "lo\nwor", "ld"].iter().enumerate() {
            f.apply(&mut v, &entry(i as u64 + 1, Event::TextDelta { text: t.to_string() }));
        }
        assert_eq!(v.transcript.len(), 2);
        let Item::Turn { role: Role::Assistant, lines, time, .. } = &v.transcript[1] else { panic!() };
        assert_eq!(lines, &["Hello", "world"]);
        assert_eq!(time, "t10");
    }

    #[test]
    fn a_tool_card_goes_from_running_to_its_result() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &entry(1, Event::TextDelta { text: "Testing.".into() }));
        f.apply(&mut v, &entry(2, call("c1")));
        assert_eq!(first_tool(&v, 0).state, ToolState::Running);
        assert_eq!(first_tool(&v, 0).args, "cargo test");
        assert_eq!(first_tool(&v, 0).icon, ToolIcon::Shell);
        let long: Vec<String> = (0..12).map(|i| format!("line {i}")).collect();
        f.apply(&mut v, &entry(4, Event::ToolResult { id: "c1".into(), output: long.join("\n"), is_error: false }));
        let card = first_tool(&v, 0);
        assert_eq!((card.state, card.outcome.as_str(), card.duration.as_str()), (ToolState::Done, "completed", "20ms"));
        let ToolBody::Output { lines, more, .. } = &card.body else { panic!() };
        assert_eq!((lines.len(), *more), (8, 4));
    }

    #[test]
    fn a_denied_call_stays_denied_when_its_error_result_arrives() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &entry(1, call("c1")));
        f.apply(&mut v, &entry(2, Event::ToolDecision { id: "c1".into(), allowed: false, by: DecidedBy::Policy, reason: Some("not here".into()) }));
        f.apply(&mut v, &entry(3, Event::ToolResult { id: "c1".into(), output: "denied: not here".into(), is_error: true }));
        let card = first_tool(&v, 0);
        assert_eq!((card.state, card.outcome.as_str()), (ToolState::Failed, "denied by policy"));
        assert!(matches!(&card.body, ToolBody::Note(n) if n == "not here"));
    }

    #[test]
    fn text_after_a_tool_opens_a_new_message() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &entry(1, call("c1")));
        f.apply(&mut v, &entry(2, Event::ToolResult { id: "c1".into(), output: "ok".into(), is_error: false }));
        f.apply(&mut v, &entry(3, Event::TextDelta { text: "Done.".into() }));
        assert_eq!(v.transcript.len(), 2);
        assert_eq!(first_tool(&v, 0).state, ToolState::Done, "the card is still found");
    }

    #[test]
    fn usage_reaches_the_status_line_and_generating_follows_the_turn() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &entry(1, Event::TurnStarted { turn: 1, provider: "anthropic".into(), model: "m".into() }));
        assert!(v.generating);
        let usage = Usage { input: 11_000, output: 1_000, cache_read: 0, cache_write: 0 };
        f.apply(&mut v, &entry(2, Event::TurnFinished { turn: 1, stop: StopReason::EndTurn, usage, cost_micro_usd: Some(1_500_000) }));
        assert!(!v.generating);
        let get = |k: &str| v.status.fields.iter().find(|(f, _)| f == k).map(|(_, x)| x.clone());
        assert_eq!(get("TOKENS").as_deref(), Some("12k / 200k"), "the context in use");
        assert_eq!(get("COST").as_deref(), Some("$1.50"));
        assert_eq!(get("ADAPTER").as_deref(), Some("anthropic"));
    }

    #[test]
    fn an_edit_shows_as_a_diff_and_arguments_show_what_matters() {
        let (mut f, mut v) = (feed(), SessionView::default());
        let input = json!({"path": "a.rs", "old_text": "one", "new_text": "two\nthree"});
        f.apply(&mut v, &entry(1, Event::ToolRequested { call: LogCall { id: "c".into(), name: "edit".into(), input }, capabilities: vec![] }));
        f.apply(&mut v, &entry(2, Event::ToolResult { id: "c".into(), output: "wrote a.rs".into(), is_error: false }));
        let card = first_tool(&v, 0);
        assert_eq!((card.args.as_str(), card.outcome.as_str(), card.diffstat), ("a.rs", "applied", Some((2, 1))));
        assert!(matches!(&card.body, ToolBody::Diff(d) if d.lines.len() == 3));
    }

    #[test]
    fn hooks_on_one_event_share_a_card() {
        let (mut f, mut v) = (feed(), SessionView::default());
        let ran = |hook: &str, outcome| Event::HookRan { point: HookPointKind::SessionStart, hook: hook.into(), outcome, detail: None };
        f.apply(&mut v, &entry(1, ran("a.sh", LogOutcome::Ok)));
        f.apply(&mut v, &entry(2, ran("b.sh", LogOutcome::Context { lines: vec!["x".into()] })));
        assert_eq!(v.transcript.len(), 1);
        let Item::Hook(h) = &v.transcript[0] else { panic!() };
        assert_eq!((h.event.as_str(), h.runs.len()), ("SessionStart", 2));
    }

    #[test]
    fn a_permission_prompt_names_the_call_and_its_needs() {
        let call = LogCall { id: "c".into(), name: "shell".into(), input: json!({"command": "rm -rf target"}) };
        let p = permission(7, &call, &[Capability::Command("rm".into())], "/work");
        assert_eq!((p.tool.as_str(), p.command.as_str(), p.scope.as_str(), p.tab), ("shell", "rm -rf target", "run rm", Some(7)));
    }
}
