//! The running session in the view: a pure mapping from log entries to transcript items
//! (IF-11, INV-5). What is on screen is a fold over the log, so a resumed session draws itself
//! the same way, and `multitool screen --scenario` shows any point of a real run.

use std::collections::HashMap;

use harness_api::event::{DecidedBy, HookOutcome as LogOutcome};
use harness_api::{Capability, Event, LogEntry, MapData, MapKind, StopReason, ToolCall as LogCall};
use serde_json::Value;

use crate::model::{
    Arrow, Branch, FlowDirection, FlowNode, Flowchart, FlowchartEvent, Gate, JournalEvent, MapCreated, MapTool, MindKind, MindLink, MindMap, MindMapEvent, MindNode, NodeStatus, Symbol,
};
use crate::model::{Activity, FlowInstanceView, Inbound, InboundKind, ThinkingView};
use crate::model::{Diff, DiffLine, HookEvent, HookOutcome, HookRun, Item, PermissionView, Role, SessionView, Tone, ToolBody, ToolCall, ToolIcon, ToolState};

const WAITING: &str = "Waiting for the model";
/// How often the spinner moves.
pub const SPIN_MS: u64 = 125;

/// Tool output lines shown before `+N more lines`.
const OUTPUT_LINES: usize = 8;

/// Turns a log time into the stamp a card shows.
pub type TimeFormat = fn(u64) -> String;

/// What the mapping remembers between entries: where each tool call's card is.
pub struct Feed {
    /// The tool a run is on, for the activity line.
    running: String,
    time: TimeFormat,
    calls: HashMap<String, CallPlace>,
    cost_micro_usd: u64,
    context_window: u64,
    known: Vec<JournalEvent>,
    /// When the reasoning being written began.
    thinking_started: Option<u64>,
    /// The instances seen of each chart (ADR 0012).
    instances: HashMap<u64, Vec<String>>,
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
        Feed { running: String::new(), time, calls: HashMap::new(), cost_micro_usd: 0, context_window, known: Vec::new(), thinking_started: None, instances: HashMap::new() }
    }

    pub fn apply(&mut self, v: &mut SessionView, entry: &LogEntry) {
        self.note_activity(v, entry);
        match &entry.event {
            Event::ToolRequested { call, .. } if call.name == "ask_user" => {
                // A slave's question is its orchestrator's to answer, not the user's (CON-100).
                if v.orchestra.as_deref() != Some("slave") {
                    v.question = harness_api::question::Question::parse(&call.input).ok().map(|q| (q, 0));
                }
            }
            // Whatever the user said next is the answer.
            Event::UserMessage { .. } => v.question = None,
            _ => {}
        }
        // It has reached the agent: its card takes over from the queue.
        let pending = match &entry.event {
            Event::UserMessage { text } => Some(harness_api::PendingMessage::User { text: text.clone() }),
            Event::Inbound { label, text } => Some(harness_api::PendingMessage::Inbound { label: label.clone(), text: text.clone() }),
            _ => None,
        };
        if let Some(message) = pending
            && let Some(at) = v.queued.iter().position(|q| q == &message)
        {
            v.queued.remove(at);
        }
        let time = (self.time)(entry.at_ms);
        match &entry.event {
            Event::UserMessage { text } => {
                v.transcript.push(Item::Turn { role: Role::User, time, lines: text.lines().map(str::to_string).collect(), attached: vec![] });
            }
            Event::Inbound { label, text } => {
                let (kind, from) = inbound_kind(label);
                v.transcript.push(Item::Inbound(Inbound { kind, from, time, text: text.clone() }));
            }
            Event::TaskState { .. } | Event::OpaqueBlock { .. } | Event::ModelToolCall { .. } => {}
            Event::TurnStarted { provider, model, .. } => {
                v.generating = true;
                set_status(v, "ADAPTER", provider);
                set_status(v, "MODEL", model);
            }
            Event::TextDelta { text } => {
                self.end_thinking(v, entry.at_ms);
                append_text(v, time, text);
            }
            Event::ThinkingDelta { text } => {
                let started = *self.thinking_started.get_or_insert(entry.at_ms);
                append_thinking(v, time, text, entry.at_ms.saturating_sub(started).div_ceil(1000));
            }
            Event::TurnFinished { stop, usage, cost_micro_usd, .. } => {
                self.end_thinking(v, entry.at_ms);
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
                self.end_thinking(v, entry.at_ms);
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
                    spin_ms: entry.at_ms,
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
                        DecidedBy::Mode => "refused in plan mode".to_string(),
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
                let memory_card = self.calls.get(id).filter(|_| !*is_error).and_then(|c| memory_card(harness_tool(&c.name), output, &time));
                let words = self.calls.get(id).filter(|_| !*is_error).and_then(|c| memory_words(harness_tool(&c.name), output));
                if let Some(card) = memory_card {
                    if let Item::Journal(j) = &card {
                        self.known.push(j.clone());
                    }
                    if let Some(slot) = self.calls.get(id).and_then(|place| slot(v, place)) {
                        *slot = card;
                    }
                    return;
                }
                if let Some(card) = self.card(v, id) {
                    let denied = card.state == ToolState::Failed;
                    card.state = if *is_error { ToolState::Failed } else { ToolState::Done };
                    if let Some(started) = started {
                        card.duration = duration(entry.at_ms.saturating_sub(started));
                    }
                    if !denied {
                        let cut_off = *is_error && output == harness_api::TOOL_STOPPED;
                        card.outcome = if cut_off { "stopped".into() } else if *is_error { "failed".into() } else { "completed".into() };
                        card.outcome_tone = if *is_error { Tone::Error } else { Tone::Success };
                        card.body = if cut_off { ToolBody::None } else { words.unwrap_or_else(|| output_body(output)) };
                        if let Some((diff, added, removed)) = diff {
                            card.outcome = "applied".into();
                            card.diffstat = Some((added, removed));
                            card.body = ToolBody::Diff(diff);
                            card.expanded = true;
                        }
                    }
                }
            }
            // The harness's own step-by-step reminders are for the model: drawn, they would be a
            // card on every message. They stay in the log.
            Event::HookRan { hook, outcome: LogOutcome::Reminder { .. } | LogOutcome::Continue { .. }, .. } if hook == "workflow-spec" || hook == "mode-guide" || hook == "repeat-guard" => {}
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
            Event::BackendSession { .. } | Event::BackendCost { .. } | Event::BackendSessionLost | Event::FormSynced { .. } => {}
            Event::ContextWindow { tokens } => self.context_window = *tokens,
            Event::Notice { label, text } => {
                let (mut kind, _) = inbound_kind(label);
                if matches!(kind, InboundKind::Voice { .. }) {
                    kind = InboundKind::Voice { duration: "not transcribed".to_string() };
                }
                v.transcript.push(Item::Inbound(Inbound { kind, from: "harness".to_string(), time, text: text.clone() }));
            }
            Event::BackgroundTasks { tasks } => {
                let known = std::mem::take(&mut v.background);
                v.background = tasks
                    .iter()
                    .map(|task| {
                        let started_ms = known.iter().find(|k| k.id == task.id).map_or(entry.at_ms, |k| k.started_ms);
                        crate::model::BackgroundView { id: task.id.clone(), description: task.description.clone(), started_ms }
                    })
                    .collect();
            }
            // Claude Code reports a command it ran in the foreground the same way; only a task on
            // the list was in the background.
            Event::BackgroundDone { id, .. } if !v.background.iter().any(|task| task.id == *id) => {}
            Event::BackgroundDone { summary, .. } => {
                let (kind, from) = inbound_kind(harness_api::HARNESS_LABEL);
                v.transcript.push(Item::Inbound(Inbound { kind, from, time, text: summary.clone() }));
            }
            Event::BackendCompacted => v.transcript.push(Item::Tips { label: "COMPACTED".into(), lines: vec!["The agent's own program compacted what it holds of this conversation.".to_string()] }),
            Event::ContextSummary { .. } => v.transcript.push(Item::Tips { label: "COMPACTED".into(), lines: vec!["What is above went into a summary here.".to_string()] }),
            Event::ConversationImported { source, title, .. } => {
                v.transcript.push(Item::Tips { label: "BROUGHT IN".into(), lines: vec![format!("\"{title}\" from {source}. It is one conversation: what is said here is in its {source} form too, and it can be opened there at any time.")] });
            }
            Event::AgentName { name, .. } => v.agent_name = Some(name.clone()),
            Event::OrchestraRole { role, .. } => {
                v.orchestra.clone_from(role);
                show_mode(v);
            }
            Event::StepDebug { on, by } => {
                v.debug_on = *on;
                show_mode(v);
                let line = if *on {
                    format!("Step-debug mode is on ({by}): the agent walks the pipeline step by step. Each finished cycle is saved to .harness/debug/runs.sqlite.")
                } else {
                    format!("Step-debug mode is off ({by}).")
                };
                v.transcript.push(Item::Tips { label: "STEP DEBUG".into(), lines: vec![line] });
            }
            Event::PlanMode { on, by } => {
                v.plan_on = *on;
                show_mode(v);
                let line = if *on {
                    format!("Plan mode is on ({by}): the agent reads and plans. Nothing in the project can change. /plan off, or approving its plan, ends it.")
                } else {
                    format!("Plan mode is off ({by}): the agent can work.")
                };
                v.transcript.push(Item::Tips { label: "PLAN MODE".into(), lines: vec![line] });
            }
            Event::Compacted { tokens_before, tokens_after, .. } => {
                let line = format!("The conversation was compacted: about {} to {} tokens. The log keeps all of it; the agent still has the journal.", short(*tokens_before), short(*tokens_after));
                v.transcript.push(Item::Tips { label: "COMPACTED".into(), lines: vec![line] });
            }
            Event::Error { source, message } => {
                v.generating = false;
                v.transcript.push(Item::Tips { label: format!("ERROR · {source}"), lines: message.lines().map(str::to_string).collect() });
            }
        }
    }

    /// A `flowstate_update` (ADR 0012): with one instance a new card per event; with several,
    /// one card each, updated in place, closed until opened, pulsing on an update.
    #[allow(clippy::too_many_arguments)]
    pub fn flow(&mut self, v: &mut SessionView, map: &MapData, instance: &str, node: u64, state: &str, off_chart: bool, path: &[u64], date: &str, now_ms: u64) {
        let event = flow_card(map, instance, node, state, off_chart, path, date);
        let seen = self.instances.entry(map.id).or_default();
        if !seen.iter().any(|i| i == instance) {
            seen.push(instance.to_string());
        }
        if seen.len() < 2 {
            v.transcript.push(Item::Flowchart(event));
            v.scroll = 0;
            return;
        }
        let existing = v.transcript.iter_mut().find_map(|it| match it {
            Item::FlowInstance(card) if card.map == map.id as u32 && card.instance == instance => Some(card),
            _ => None,
        });
        match existing {
            Some(card) => {
                card.event = event;
                if !card.expanded {
                    card.pulse_until_ms = Some(now_ms + PULSE_MS);
                }
            }
            None => v.transcript.push(Item::FlowInstance(FlowInstanceView {
                map: map.id as u32,
                instance: instance.to_string(),
                event,
                expanded: false,
                pulse_until_ms: Some(now_ms + PULSE_MS),
                pulse: 0,
            })),
        }
    }

    /// Journal entries seen since the last call, for the view's unfolding tickets.
    pub fn take_known(&mut self) -> Vec<JournalEvent> {
        std::mem::take(&mut self.known)
    }

    fn end_thinking(&mut self, v: &mut SessionView, at_ms: u64) {
        if let Some(started) = self.thinking_started.take() {
            finish_thinking(v, at_ms.saturating_sub(started).div_ceil(1000));
        }
    }

    /// The run has ended, however it ended: nothing is generating any more.
    /// What the status line's TOKENS is out of, once the model changes.
    pub fn set_context_window(&mut self, window: u64) {
        self.context_window = window;
    }

    /// The activity on the status line follows the log.
    fn note_activity(&mut self, v: &mut SessionView, entry: &LogEntry) {
        let state = match &entry.event {
            Event::UserMessage { .. } | Event::Inbound { .. } => WAITING.to_string(),
            Event::TurnStarted { model, .. } => format!("Waiting for {model}"),
            Event::ThinkingDelta { .. } => "Thinking".to_string(),
            Event::TextDelta { .. } => "Writing".to_string(),
            Event::ToolRequested { call, .. } | Event::ModelToolCall { call } => {
                self.running = call.name.clone();
                format!("Calling {}", tool_title(&call.name).0)
            }
            Event::ToolDecision { allowed: true, .. } => format!("Calling {}", tool_title(&self.running).0),
            Event::ToolResult { .. } => WAITING.to_string(),
            Event::Verification { .. } => "Checking the result".to_string(),
            Event::TaskState { to: harness_api::TaskState::Verifying, .. } => "Checking the result".to_string(),
            Event::Compacted { .. } => "Compacting the conversation".to_string(),
            _ => return,
        };
        let activity = v.activity.get_or_insert_with(|| Activity { started_ms: entry.at_ms, now_ms: entry.at_ms, ..Default::default() });
        activity.state = state;
    }

    /// A log read back: the process that held its background tasks is gone.
    pub fn replayed(&mut self, v: &mut SessionView) {
        v.background.clear();
        self.run_ended(v);
    }

    pub fn run_ended(&mut self, v: &mut SessionView) {
        v.activity = None;
        if self.thinking_started.take().is_some() {
            let secs = live_thinking(v).map_or(0, |t| t.secs);
            finish_thinking(v, secs);
        }
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

fn slot<'a>(v: &'a mut SessionView, place: &CallPlace) -> Option<&'a mut Item> {
    match v.transcript.get_mut(place.item)? {
        Item::Turn { attached, .. } => attached.get_mut(place.attached),
        _ => None,
    }
}

/// A tool of the harness's own, by the name its loop knows it, whichever way it was reached: a
/// delegate has them over MCP. Another server's tool of that name is not one of them.
fn harness_tool(name: &str) -> &str {
    match tool_title(name) {
        (tool, None | Some("harness")) => tool,
        _ => name,
    }
}

/// What a memory tool answered with no card of its own, in words: its JSON is for the model.
fn memory_words(tool: &str, output: &str) -> Option<ToolBody> {
    const SHOWN: usize = 5;
    let answer: Value = serde_json::from_str(output).ok()?;
    let entry_line = |e: &Value| {
        let text = e["body"].as_str().or(e["excerpt"].as_str()).unwrap_or_default();
        let first = text.lines().next().unwrap_or_default();
        let cut = if first.chars().count() > 100 { format!("{}…", first.chars().take(99).collect::<String>()) } else { first.to_string() };
        format!("#{}  {}  {cut}", e["id"], e["category"].as_str().unwrap_or_default())
    };
    let listed = |entries: Vec<&Value>, total: usize| {
        if entries.is_empty() {
            return ToolBody::Note("Nothing found.".to_string());
        }
        let lines: Vec<String> = entries.iter().take(SHOWN).map(|e| entry_line(e)).collect();
        let more = total.saturating_sub(lines.len());
        ToolBody::Output { label: None, lines, numbered: false, more }
    };
    Some(match tool {
        "journal_search" | "journal_list" => {
            let entries: Vec<&Value> = answer["entries"].as_array()?.iter().collect();
            let total = answer["total"].as_u64().map_or(entries.len(), |t| t as usize);
            match (listed(entries, total), answer["linked_total"].as_u64()) {
                (ToolBody::Output { lines, numbered, more, .. }, Some(linked)) if linked > 0 => ToolBody::Output { label: Some(format!("{linked} linked")), lines, numbered, more },
                (body, _) => body,
            }
        }
        "journal_branch" => {
            let entries: Vec<&Value> = answer["entries"].as_array()?.iter().map(|e| &e["entry"]).collect();
            let total = entries.len();
            let ToolBody::Output { lines, more, .. } = listed(entries, total) else { return None };
            ToolBody::Output { label: Some(answer["state"].as_str().unwrap_or_default().to_string()), lines, numbered: false, more }
        }
        "journal_agenda" => {
            let groups = answer["agenda"].as_array()?;
            if groups.is_empty() {
                ToolBody::Note("Nothing open.".to_string())
            } else {
                ToolBody::Note(groups.iter().map(|g| format!("{} {}", g["flag"].as_str().unwrap_or_default(), g["total"])).collect::<Vec<_>>().join("  ·  "))
            }
        }
        "journal_tags" => {
            let names = |list: &Value| list.as_array().map_or(String::new(), |all| all.iter().take(12).map(|t| format!("{} {}", t["name"].as_str().unwrap_or_default(), t["count"])).collect::<Vec<_>>().join("  ·  "));
            ToolBody::Note(names(&answer["tags"]))
        }
        "journal_close" | "journal_mark" | "journal_reopen" | "journal_abandon" | "journal_link" | "map_edit" | "map_node_state" | "map_link" | "map_ref" | "map_close" => ToolBody::Note(summary(&answer)),
        _ => return None,
    })
}

/// A memory tool's result as its own card: JOURNAL, MINDMAP or FLOWCHART (ADR 0011).
fn memory_card(tool: &str, output: &str, date: &str) -> Option<Item> {
    match tool {
        "journal_add" | "journal_get" => {
            let r: harness_api::JournalRecord = serde_json::from_str(output).ok()?;
            Some(Item::Journal(JournalEvent { id: r.id as u32, category: r.category, tags: r.tags, date: date.to_string(), text: r.body }))
        }
        "mindmap_new" | "flowchart_new" => {
            let m: MapData = serde_json::from_str(output).ok()?;
            let map_tool = if m.kind == MapKind::Flowchart { MapTool::Flowchart } else { MapTool::MindMap };
            Some(Item::MapCreated(MapCreated { tool: map_tool, id: m.id as u32, title: m.title, shared: true }))
        }
        "mindmap_add" | "mindmap_show" => {
            let m: MapData = serde_json::from_str(output).ok()?;
            let change = if tool == "mindmap_add" { "+node".to_string() } else { "shown".to_string() };
            Some(Item::MindMap(MindMapEvent { id: m.id as u32, change, date: date.to_string(), map: mind_map(&m, tool == "mindmap_add"), open: None, focus: None, pan: (0, 0) }))
        }
        "flowchart_add" | "flowchart_show" => {
            let m: MapData = serde_json::from_str(output).ok()?;
            let change = if tool == "flowchart_add" { "+node".to_string() } else { "shown".to_string() };
            Some(Item::Flowchart(FlowchartEvent { id: m.id as u32, change, date: date.to_string(), chart: flowchart(&m, tool == "flowchart_add"), open: None, focus: None, pan: (0, 0) }))
        }
        _ => None,
    }
}

fn node_id(id: u64) -> String {
    format!("n{id}")
}

fn refs(r: &[u64]) -> Vec<u32> {
    r.iter().map(|x| *x as u32).collect()
}

fn mind_map(m: &MapData, newest_changed: bool) -> MindMap {
    let newest = m.nodes.iter().map(|n| n.id).max();
    MindMap {
        nodes: m
            .nodes
            .iter()
            .map(|n| MindNode {
                id: node_id(n.id),
                text: n.text.clone(),
                kind: match n.kind.as_str() {
                    "topic" => MindKind::Topic,
                    "goal" => MindKind::Goal,
                    "decision" => MindKind::Decision,
                    "fact" => MindKind::Fact,
                    "hypothesis" => MindKind::Hypothesis,
                    "question" => MindKind::Question,
                    "task" => MindKind::Task,
                    "idea" => MindKind::Idea,
                    "dead-end" => MindKind::DeadEnd,
                    _ => MindKind::Note,
                },
                status: match n.state.as_str() {
                    "done" => NodeStatus::Done,
                    "dropped" => NodeStatus::Dropped,
                    _ => NodeStatus::Open,
                },
                refs: refs(&n.refs),
                flows: vec![],
                changed: newest_changed && Some(n.id) == newest,
            })
            .collect(),
        branches: m.nodes.iter().filter_map(|n| n.parent.map(|p| Branch { from: node_id(p), to: node_id(n.id), flows: vec![] })).collect(),
        links: m.edges.iter().map(|e| MindLink { from: node_id(e.from), to: node_id(e.to), label: e.label.clone(), flows: vec![] }).collect(),
    }
}

fn flowchart(m: &MapData, newest_changed: bool) -> Flowchart {
    let newest = m.nodes.iter().map(|n| n.id).max();
    Flowchart {
        direction: if m.direction == "down" { FlowDirection::Down } else { FlowDirection::Right },
        nodes: m
            .nodes
            .iter()
            .map(|n| FlowNode {
                id: node_id(n.id),
                text: n.text.clone(),
                symbol: match n.kind.as_str() {
                    // A chart's root is its starting point.
                    "start" | "topic" => Symbol::Start,
                    "end" => Symbol::End,
                    "decision" => Symbol::Decision,
                    "io" => Symbol::Io,
                    "subprocess" => Symbol::Subprocess,
                    "document" => Symbol::Document,
                    "data" => Symbol::Data,
                    "manual" => Symbol::Manual,
                    "prep" => Symbol::Prep,
                    "connector" => Symbol::Connector,
                    "delay" => Symbol::Delay,
                    "note" => Symbol::Note,
                    "and" => Symbol::Gate(Gate::And),
                    "or" => Symbol::Gate(Gate::Or),
                    "not" => Symbol::Gate(Gate::Not),
                    "nand" => Symbol::Gate(Gate::Nand),
                    "nor" => Symbol::Gate(Gate::Nor),
                    "xor" => Symbol::Gate(Gate::Xor),
                    "xnor" => Symbol::Gate(Gate::Xnor),
                    _ => Symbol::Process,
                },
                refs: refs(&n.refs),
                changed: newest_changed && Some(n.id) == newest,
                active: false,
            })
            .collect(),
        arrows: m.edges.iter().map(|e| Arrow { from: node_id(e.from), to: node_id(e.to), label: (!e.label.is_empty()).then(|| e.label.clone()), refs: refs(&e.refs) }).collect(),
    }
}

/// How long a pulse lasts.
pub const PULSE_MS: u64 = 1000;

/// Moves the running pulses on: soft, up and down by colour. Returns whether any is still alive,
/// so the app knows to keep drawing.
/// Keeps the activity line going: it starts with the run, before the log has said anything, moves
/// on eight times a second, and redraws nothing between runs. True if it changed.
pub fn tick_activity(v: &mut SessionView, now_ms: u64) -> bool {
    let background = !v.background.is_empty() && v.background_now_ms / SPIN_MS != now_ms / SPIN_MS;
    if background {
        v.background_now_ms = now_ms;
    }
    if !v.generating {
        return v.activity.take().is_some() | background;
    }
    let activity = v.activity.get_or_insert_with(|| Activity { state: WAITING.to_string(), started_ms: now_ms, now_ms: 0 });
    if activity.now_ms / SPIN_MS == now_ms / SPIN_MS {
        return background;
    }
    activity.now_ms = now_ms;
    for item in &mut v.transcript {
        spin_tools(item, now_ms);
    }
    true
}

/// The running calls' spinners, wherever in the transcript they are.
fn spin_tools(item: &mut Item, now_ms: u64) {
    match item {
        Item::Tool(call) if call.state == ToolState::Running => call.spin_ms = now_ms,
        Item::Turn { attached, .. } => attached.iter_mut().for_each(|a| spin_tools(a, now_ms)),
        _ => {}
    }
}

pub fn tick_pulses(v: &mut SessionView, now_ms: u64) -> bool {
    let mut any = false;
    for item in &mut v.transcript {
        let Item::FlowInstance(card) = item else { continue };
        let Some(until) = card.pulse_until_ms else { continue };
        if now_ms >= until {
            card.pulse_until_ms = None;
            card.pulse = 0;
            any = true;
            continue;
        }
        let elapsed = PULSE_MS.saturating_sub(until - now_ms) as f32 / PULSE_MS as f32;
        card.pulse = (8.0 * (1.0 - (2.0 * elapsed - 1.0).abs())).round() as u8;
        any = true;
    }
    any
}

/// A `journal` callback's entry as its card (no agent turn).
pub fn journal_card(r: &harness_api::JournalRecord, date: &str) -> JournalEvent {
    JournalEvent { id: r.id as u32, category: r.category.clone(), tags: r.tags.clone(), date: date.to_string(), text: r.body.clone() }
}

/// A `flowstate_update` as a FLOWCHART card: the chart with the event's node active (ADR 0012).
/// The chart is the reference; an off-chart node is said so and marks nothing.
pub fn flow_card(map: &MapData, instance: &str, node: u64, state: &str, off_chart: bool, path: &[u64], date: &str) -> FlowchartEvent {
    let mut chart = flowchart(map, false);
    let id = node_id(node);
    for n in &mut chart.nodes {
        n.active = !off_chart && n.id == id;
        n.changed = path.iter().any(|p| node_id(*p) == n.id) && !n.active;
    }
    let change = if off_chart { format!("{instance}: off-chart event at n{node} ({state})") } else { format!("{instance}: n{node} {state}") };
    FlowchartEvent { id: map.id as u32, change, date: date.to_string(), chart, open: None, focus: None, pan: (0, 0) }
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
    // A plan waiting for approval reads as one, not as a tool (CON-163).
    let plan = call.name.ends_with("plan_exit");
    PermissionView {
        tool: call.name.clone(),
        summary: if plan { "The agent asks you to approve its plan: approve leaves plan mode and starts the work, deny keeps planning. It is asked on Telegram too; the first answer decides.".to_string() } else { format!("The agent wants to use the tool {}.", call.name) },
        command: summary(&call.input),
        workdir: workdir.to_string(),
        scope: if needs.is_empty() { "nothing declared".into() } else { needs.join(" · ") },
        policy: "approval required".into(),
        choice: 0,
        tab: Some(tab),
        call_id: call.id.clone(),
        locked: true,
    }
}

/// The live reasoning of the newest turn, if any.
fn live_thinking(v: &mut SessionView) -> Option<&mut ThinkingView> {
    let Some(Item::Turn { attached, .. }) = v.transcript.last_mut() else { return None };
    attached.iter_mut().find_map(|a| match a {
        Item::Thinking(t) if t.live => Some(t),
        _ => None,
    })
}

/// The reasoning of the running turn grows one block, inside the turn, above its answer.
fn append_thinking(v: &mut SessionView, time: String, text: &str, secs: u64) {
    if live_thinking(v).is_none() {
        // Reasoning with nothing to show has no card: the status line says it is on.
        if text.trim().is_empty() {
            return;
        }
        let reusable = matches!(v.transcript.last(), Some(Item::Turn { role: Role::Assistant, lines, attached, .. }) if attached.is_empty() && lines.iter().all(|l| l.is_empty()));
        if !reusable {
            v.transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec![], attached: vec![] });
        }
        if let Some(Item::Turn { attached, .. }) = v.transcript.last_mut() {
            attached.push(Item::Thinking(ThinkingView { lines: vec![String::new()], live: true, secs, expanded: false }));
        }
    }
    let Some(t) = live_thinking(v) else { return };
    t.secs = secs;
    let mut parts = text.split('\n');
    if let (Some(first), Some(last)) = (parts.next(), t.lines.last_mut()) {
        last.push_str(first);
    }
    t.lines.extend(parts.map(str::to_string));
}

/// The answer or a tool has begun: the reasoning folds to its one line.
fn finish_thinking(v: &mut SessionView, secs: u64) {
    if let Some(t) = live_thinking(v) {
        t.live = false;
        t.secs = secs;
    }
}

fn append_text(v: &mut SessionView, time: String, text: &str) {
    // Text after a tool call starts a new message under it, in reading order.
    let open = matches!(v.transcript.last(), Some(Item::Turn { role: Role::Assistant, attached, .. }) if attached.iter().all(|a| matches!(a, Item::Thinking(_))));
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

/// A voice message from Telegram, transcribed: its label in the log.
const VOICE_LABEL: &str = harness_api::TELEGRAM_VOICE_LABEL;

/// A tab goes by the name its agent goes by, however it was given: /nick, the agent's own tool, or
/// Telegram. All of them reach here as the same event.
pub fn name_tab(tab: &mut crate::model::Tab) {
    if let Some(name) = &tab.view.agent_name
        && tab.title != *name
    {
        tab.title.clone_from(name);
    }
}

/// What a message's label says about where it came from: the card it gets, and who it is from.
pub fn inbound_kind(label: &str) -> (InboundKind, String) {
    if label == harness_api::TELEGRAM_LABEL {
        return (InboundKind::Telegram, "operator".to_string());
    }
    if label == harness_api::HARNESS_LABEL {
        return (InboundKind::System, "harness".to_string());
    }
    if label == VOICE_LABEL {
        return (InboundKind::Voice { duration: "transcribed".to_string() }, "operator".to_string());
    }
    if label == "game" {
        return (InboundKind::Game { table: String::new() }, "the game table".to_string());
    }
    if let Some(nick) = label.strip_prefix("agent:") {
        return (InboundKind::Agent { session: nick.to_string(), route: None }, nick.to_string());
    }
    if let Some(entry) = label.strip_prefix("reminder:") {
        return (InboundKind::Reminder { id: entry.parse().unwrap_or(0), every: String::new() }, String::new());
    }
    (InboundKind::Callback { transport: "HTTP".into(), status: "200".into() }, label.to_string())
}

/// A message a launcher typed in, by the prefix it comes with: its label and its text. Only the start
/// counts, and several waiting messages are one: a line inside a body cannot pass for a sender.
pub fn parse_inbound(pasted: &str) -> Option<(String, String)> {
    let (head, body) = pasted.strip_prefix('[')?.split_once("]: ")?;
    let label = match head {
        "telegram_message" => harness_api::TELEGRAM_LABEL.to_string(),
        "telegram_voice_message" => VOICE_LABEL.to_string(),
        "callback" => "callback".to_string(),
        _ => {
            if let Some(nick) = head.strip_prefix("agent-to-agent:") {
                format!("agent:{nick}")
            } else if let Some(entry) = head.strip_prefix("reminder #") {
                format!("reminder:{entry}")
            } else if head.starts_with("callback:") || head.starts_with("pool-job:") {
                head.to_string()
            } else {
                return None;
            }
        }
    };
    Some((label, body.to_string()))
}

/// The status line counts against this window from now on.
pub fn show_context_window(v: &mut SessionView, window: u64) {
    let used = v.status.fields.iter().find(|(k, _)| k == "TOKENS").map(|(_, value)| value.split(" / ").next().unwrap_or("0").to_string());
    if let Some(used) = used {
        set_status(v, "TOKENS", &format!("{used} / {}", short(window)));
    }
}

/// What mode this conversation runs in, first on the status line.
pub fn show_mode(v: &mut SessionView) {
    let mut modes: Vec<&str> = Vec::new();
    match v.orchestra.as_deref() {
        Some("master") => modes.push("orchestrate"),
        Some("slave") => modes.push("slave"),
        _ => {}
    }
    if v.plan_on {
        modes.push("plan");
    }
    if v.debug_on {
        modes.push("debug");
    }
    let mode = if modes.is_empty() { "normal".to_string() } else { modes.join(" + ") };
    v.status.fields.retain(|(key, _)| key != "MODE");
    v.status.fields.insert(0, ("MODE".to_string(), mode));
}

pub fn set_status(v: &mut SessionView, key: &str, value: &str) {
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
    let shown = all.iter().take(OUTPUT_LINES).map(|l| expand_tabs(l)).collect();
    ToolBody::Output { label: None, lines: shown, numbered: false, more: all.len().saturating_sub(OUTPUT_LINES) }
}

/// A terminal's tabs as the spaces they stand for: a drawn tab takes no cell.
fn expand_tabs(line: &str) -> String {
    const STOP: usize = 8;
    let mut out = String::with_capacity(line.len());
    let mut column = 0;
    for c in line.chars() {
        if c == '\t' {
            let spaces = STOP - column % STOP;
            out.extend(std::iter::repeat_n(' ', spaces));
            column += spaces;
        } else {
            out.push(c);
            column += crate::markdown::cell_width(c);
        }
    }
    out
}

fn hook_outcome(outcome: &LogOutcome) -> HookOutcome {
    match outcome {
        LogOutcome::Ok => HookOutcome::Ok,
        LogOutcome::Context { lines } | LogOutcome::Reminder { lines } | LogOutcome::Continue { lines } => HookOutcome::Context(lines.clone()),
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
/// A tool's name as it is drawn: an MCP tool's own name, and the server it is on. The log keeps
/// the real one.
pub fn tool_title(name: &str) -> (&str, Option<&str>) {
    if let Some(tool) = name.strip_prefix("mode__") {
        return (tool, Some("mode"));
    }
    name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")).map_or((name, None), |(server, tool)| (tool, Some(server)))
}

/// A tool's arguments in words: the one that says what it does, or each by its name.
fn summary(input: &Value) -> String {
    const MAIN_FIELDS: [&str; 7] = ["command", "file_path", "path", "url", "text", "query", "pattern"];
    let main = MAIN_FIELDS.iter().find_map(|f| input[*f].as_str()).map(str::to_string);
    let text = match (main, input) {
        (Some(main), _) => main,
        (None, Value::Object(fields)) if fields.len() == 1 && fields.values().all(Value::is_string) => fields.values().next().and_then(Value::as_str).unwrap_or_default().to_string(),
        (None, Value::Object(fields)) => fields.iter().map(|(name, value)| format!("{name}: {}", argument(value))).collect::<Vec<_>>().join(", "),
        (None, Value::Null) => String::new(),
        (None, other) => argument(other),
    };
    let line = text.lines().next().unwrap_or_default();
    if line.chars().count() > 100 { format!("{}…", line.chars().take(99).collect::<String>()) } else { line.to_string() }
}

fn argument(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => format!("{} items", items.len()),
        Value::Object(_) => "…".to_string(),
        other => other.to_string(),
    }
}

fn short(n: u64) -> String {
    match n {
        1_000_000.. if n.is_multiple_of(1_000_000) => format!("{}M", n / 1_000_000),
        1000.. => format!("{}k", n / 1000),
        _ => n.to_string(),
    }
}

fn duration(ms: u64) -> String {
    if ms >= 1000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{ms}ms") }
}

#[cfg(test)]
mod activity_tests {
    use super::*;
    use harness_api::event::LogEntry as Entry;

    fn at(at_ms: u64, event: Event) -> Entry {
        Entry { seq: 0, at_ms, event }
    }

    #[test]
    fn a_tab_goes_by_the_name_its_agent_was_given() {
        let mut tab = crate::model::Tab::new("4f7e3c2a", crate::model::TabIcon::Prompt, crate::model::TabMark::None, Default::default());
        name_tab(&mut tab);
        assert_eq!(tab.title, "4f7e3c2a", "no name yet");
        let mut feed = Feed::new(|_| String::new(), 1000);
        feed.apply(&mut tab.view, &LogEntry { seq: 1, at_ms: 0, event: Event::AgentName { name: "multitool-arch".into(), by: "agent".into() } });
        name_tab(&mut tab);
        assert_eq!(tab.title, "multitool-arch");
    }

    #[test]
    fn the_line_follows_the_run_and_counts_from_its_start() {
        let mut f = Feed::new(|_| String::new(), 1000);
        let mut v = SessionView::default();
        assert!(!tick_activity(&mut v, 500), "idle: nothing to redraw");
        v.generating = true;
        assert!(tick_activity(&mut v, 1000));
        assert_eq!(v.activity.as_ref().map(|a| (a.state.as_str(), a.started_ms)), Some(("Waiting for the model", 1000)));
        assert!(!tick_activity(&mut v, 1050), "same frame, no redraw");
        assert!(tick_activity(&mut v, 1200));

        let state = |v: &SessionView| v.activity.as_ref().map(|a| a.state.clone()).unwrap();
        let call = LogCall { id: "c".into(), name: "shell".into(), input: serde_json::json!({}) };
        f.apply(&mut v, &at(1200, Event::TurnStarted { turn: 1, provider: "p".into(), model: "m1".into() }));
        assert_eq!(state(&v), "Waiting for m1");
        f.apply(&mut v, &at(1300, Event::ThinkingDelta { text: "hm".into() }));
        assert_eq!(state(&v), "Thinking");
        f.apply(&mut v, &at(1400, Event::TextDelta { text: "ok".into() }));
        assert_eq!(state(&v), "Writing");
        f.apply(&mut v, &at(1500, Event::ToolRequested { call: call.clone(), capabilities: vec![] }));
        assert_eq!(state(&v), "Calling shell");
        v.activity.as_mut().unwrap().state = "Waiting for your answer".into();
        f.apply(&mut v, &at(1600, Event::ToolDecision { id: "c".into(), allowed: true, by: harness_api::event::DecidedBy::User, reason: None }));
        assert_eq!(state(&v), "Calling shell");
        f.apply(&mut v, &at(1700, Event::ToolResult { id: "c".into(), output: "x".into(), is_error: false }));
        assert_eq!(state(&v), "Waiting for the model");
        assert_eq!(v.activity.as_ref().unwrap().started_ms, 1000, "the count is the run's, not the state's");

        f.run_ended(&mut v);
        assert!(v.activity.is_none());
    }

    #[test]
    fn an_old_log_replayed_leaves_no_line() {
        let mut f = Feed::new(|_| String::new(), 1000);
        let mut v = SessionView::default();
        f.apply(&mut v, &at(5, Event::UserMessage { text: "hi".into() }));
        f.run_ended(&mut v);
        assert!(v.activity.is_none());
        assert!(!tick_activity(&mut v, 99_000));
    }
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
    fn memory_tools_draw_their_own_cards() {
        let (mut f, mut v) = (feed(), SessionView::default());
        let run = |f: &mut Feed, v: &mut SessionView, id: &str, name: &str, output: serde_json::Value| {
            f.apply(v, &entry(1, Event::ToolRequested { call: LogCall { id: id.into(), name: name.into(), input: json!({}) }, capabilities: vec![] }));
            f.apply(v, &entry(2, Event::ToolResult { id: id.into(), output: output.to_string(), is_error: false }));
        };
        run(&mut f, &mut v, "a", "journal_add", json!({"id": 7, "created": "", "category": "decision", "tags": ["x"], "body": "why", "flags": []}));
        let map = json!({"id": 1, "title": "t", "kind": "flowchart", "direction": "down", "root": 1,
            "nodes": [{"id": 1, "parent": null, "kind": "start", "text": "go", "state": "open", "refs": []}, {"id": 2, "parent": 1, "kind": "and", "text": "both", "state": "open", "refs": [7]}],
            "edges": [{"from": 1, "to": 2, "label": "yes", "refs": []}]});
        run(&mut f, &mut v, "b", "flowchart_new", map.clone());
        run(&mut f, &mut v, "c", "flowchart_add", map.clone());
        let mut mind = map.clone();
        mind["kind"] = json!("mindmap");
        mind["nodes"][1]["kind"] = json!("dead-end");
        run(&mut f, &mut v, "d", "mindmap_add", mind);

        let Item::Turn { attached, .. } = &v.transcript[0] else { panic!() };
        assert!(matches!(&attached[0], Item::Journal(j) if j.id == 7 && j.text == "why"));
        assert!(matches!(&attached[1], Item::MapCreated(m) if m.tool == MapTool::Flowchart));
        let Item::Flowchart(chart) = &attached[2] else { panic!() };
        assert_eq!(chart.chart.direction, FlowDirection::Down);
        assert_eq!(chart.chart.nodes[1].symbol, Symbol::Gate(Gate::And));
        assert_eq!(chart.chart.arrows[0].label.as_deref(), Some("yes"));
        assert!(chart.chart.nodes[1].changed && !chart.chart.nodes[0].changed);
        let Item::MindMap(m) = &attached[3] else { panic!() };
        assert_eq!(m.map.nodes[1].kind, MindKind::DeadEnd);
        assert_eq!(m.map.branches.len(), 1);
        assert_eq!(f.take_known().len(), 1);
        assert!(f.take_known().is_empty());
    }

    fn thinking_of(v: &SessionView, turn: usize) -> ThinkingView {
        let Item::Turn { attached, .. } = &v.transcript[turn] else { panic!("not a turn") };
        attached.iter().find_map(|a| if let Item::Thinking(t) = a { Some(t.clone()) } else { None }).expect("a reasoning block")
    }

    fn at(ms: u64, event: Event) -> LogEntry {
        LogEntry { seq: ms, at_ms: ms, event }
    }

    #[test]
    fn reasoning_lives_in_the_turn_then_folds_when_the_answer_starts() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &at(1000, Event::ThinkingDelta { text: "The ".into() }));
        f.apply(&mut v, &at(3500, Event::ThinkingDelta { text: "user\nasks".into() }));
        let live = thinking_of(&v, 0);
        assert!(live.live);
        assert_eq!((live.secs, live.lines.as_slice()), (3, &["The user".to_string(), "asks".to_string()][..]));
        f.apply(&mut v, &at(5000, Event::TextDelta { text: "Four.".into() }));
        assert_eq!(v.transcript.len(), 1, "the answer joins the same turn");
        let done = thinking_of(&v, 0);
        assert!(!done.live && !done.expanded);
        assert_eq!(done.secs, 4);
        let Item::Turn { lines, .. } = &v.transcript[0] else { panic!() };
        assert_eq!(lines, &["Four."]);
    }

    #[test]
    fn a_tool_or_the_end_of_a_run_folds_reasoning_too() {
        let (mut f, mut v) = (feed(), SessionView::default());
        f.apply(&mut v, &at(0, Event::ThinkingDelta { text: "hm".into() }));
        f.apply(&mut v, &at(2000, call("c1")));
        assert!(!thinking_of(&v, 0).live);
        assert_eq!(first_tool_after_thinking(&v).name, "shell");
        f.apply(&mut v, &at(2100, Event::ToolResult { id: "c1".into(), output: "ok".into(), is_error: false }));
        // The next turn thinks anew, in a new message.
        f.apply(&mut v, &at(3000, Event::ThinkingDelta { text: "next".into() }));
        assert_eq!(v.transcript.len(), 2);
        assert!(thinking_of(&v, 1).live);
        f.run_ended(&mut v);
        assert!(!thinking_of(&v, 1).live, "a stopped run leaves nothing live");
    }

    fn first_tool_after_thinking(v: &SessionView) -> &ToolCall {
        let Item::Turn { attached, .. } = &v.transcript[0] else { panic!() };
        attached.iter().find_map(|a| if let Item::Tool(c) = a { Some(c) } else { None }).expect("a tool")
    }

    fn chart() -> MapData {
        serde_json::from_value(json!({"id": 1, "title": "t", "kind": "flowchart", "direction": "down", "root": 1,
            "nodes": [{"id": 1, "parent": null, "kind": "start", "text": "go", "state": "open", "refs": []}, {"id": 2, "parent": 1, "kind": "process", "text": "work", "state": "open", "refs": []}],
            "edges": [{"from": 1, "to": 2, "label": "", "refs": []}]})).unwrap()
    }

    #[test]
    fn one_instance_adds_cards_and_several_share_one_each_in_place() {
        let (mut f, mut v) = (feed(), SessionView::default());
        let m = chart();
        f.flow(&mut v, &m, "a", 1, "entered", false, &[1], "d", 0);
        f.flow(&mut v, &m, "a", 2, "entered", false, &[1, 2], "d", 10);
        assert_eq!(v.transcript.iter().filter(|i| matches!(i, Item::Flowchart(_))).count(), 2, "one instance: a card per event");

        f.flow(&mut v, &m, "b", 1, "entered", false, &[1], "d", 20);
        f.flow(&mut v, &m, "b", 2, "entered", false, &[1, 2], "d", 30);
        f.flow(&mut v, &m, "a", 2, "failed", false, &[1, 2], "d", 40);
        let cards: Vec<&FlowInstanceView> = v.transcript.iter().filter_map(|i| if let Item::FlowInstance(c) = i { Some(c) } else { None }).collect();
        assert_eq!(cards.len(), 2, "one card per instance");
        assert_eq!(cards[0].instance, "b");
        assert!(cards[0].event.change.contains("n2"), "updated in place: {}", cards[0].event.change);
        assert!(cards.iter().all(|c| !c.expanded), "closed by default");
        assert!(cards[1].event.change.contains("failed"));
        assert_eq!(v.transcript.len(), 4, "history stays, nothing else added");
    }

    #[test]
    fn a_pulse_rises_falls_and_ends_and_an_open_card_has_none() {
        let (mut f, mut v) = (feed(), SessionView::default());
        let m = chart();
        f.flow(&mut v, &m, "a", 1, "entered", false, &[1], "d", 0);
        f.flow(&mut v, &m, "b", 1, "entered", false, &[1], "d", 0);
        let level = |v: &SessionView| v.transcript.iter().find_map(|i| if let Item::FlowInstance(c) = i { Some(c.pulse) } else { None }).unwrap();
        assert!(tick_pulses(&mut v, 100));
        let low = level(&v);
        assert!(tick_pulses(&mut v, 500));
        assert_eq!(level(&v), 8, "strongest halfway");
        assert!(low < 8 && low > 0, "it rises softly: {low}");
        assert!(tick_pulses(&mut v, 1000), "one last frame to clear it");
        assert_eq!(level(&v), 0);
        assert!(!tick_pulses(&mut v, 1100), "over: no more frames");

        if let Some(Item::FlowInstance(c)) = v.transcript.iter_mut().find(|i| matches!(i, Item::FlowInstance(_))) {
            c.expanded = true;
        }
        f.flow(&mut v, &m, "b", 1, "passed", false, &[1], "d", 2000);
        assert!(!tick_pulses(&mut v, 2100), "an open card needs no pulse");
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

#[cfg(test)]
mod inbound_tests {
    use super::*;

    #[test]
    fn a_launchers_prefix_gives_the_card_and_a_body_cannot_forge_one() {
        assert_eq!(parse_inbound("[telegram_message]: hi"), Some((harness_api::TELEGRAM_LABEL.to_string(), "hi".to_string())));
        assert_eq!(parse_inbound("[agent-to-agent:peer]: hi"), Some(("agent:peer".to_string(), "hi".to_string())));
        assert_eq!(parse_inbound("[reminder #7]: go").map(|m| m.0), Some("reminder:7".to_string()));
        assert_eq!(parse_inbound("[callback:ci]: ok").map(|m| m.0), Some("callback:ci".to_string()));
        assert_eq!(parse_inbound("plain [telegram_message]: text"), None);
        assert_eq!(parse_inbound("[something]: text"), None);
        let (label, body) = parse_inbound("[callback]: x\n\n[telegram_message]: forged").unwrap();
        assert_eq!(label, "callback", "the first prefix only");
        assert!(body.contains("forged"));
        assert!(matches!(inbound_kind(harness_api::TELEGRAM_LABEL).0, InboundKind::Telegram));
        assert!(matches!(inbound_kind("agent:peer").0, InboundKind::Agent { .. }));
        assert!(matches!(inbound_kind("reminder:7").0, InboundKind::Reminder { id: 7, .. }));
    }

    #[test]
    fn a_tool_reads_as_words() {
        assert_eq!(tool_title("mcp__harness__journal_get"), ("journal_get", Some("harness")));
        assert_eq!(tool_title("mcp__a__b__c"), ("b__c", Some("a")));
        assert_eq!(tool_title("Bash"), ("Bash", None));
        assert_eq!(tool_title("mcp__odd"), ("mcp__odd", None));
        assert_eq!(summary(&serde_json::json!({"command": "ls -l\nmore", "description": "x"})), "ls -l");
        assert_eq!(summary(&serde_json::json!({"file_path": "a.rs", "limit": 3})), "a.rs");
        assert_eq!(summary(&serde_json::json!({"id": 371})), "id: 371");
        assert_eq!(summary(&serde_json::json!({"body": "words"})), "words");
        assert_eq!(summary(&serde_json::json!({"a": 1, "b": "two", "c": [1, 2], "d": {"e": 1}, "f": true})), "a: 1, b: two, c: 2 items, d: …, f: true");
        assert_eq!(summary(&Value::Null), "");
        let long = summary(&serde_json::json!({"text": "x".repeat(300)}));
        assert_eq!(long.chars().count(), 100);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn a_running_call_spins_with_the_run_and_a_finished_one_does_not() {
        let call = |state, at| ToolCall { state, spin_ms: at, ..crate::debug::tool("x", "", ToolIcon::None, state, "", "", ToolBody::None) };
        let mut v = SessionView {
            generating: true,
            transcript: vec![Item::Tool(call(ToolState::Running, 0)), Item::Tool(call(ToolState::Done, 7)), Item::Turn { role: Role::Assistant, time: String::new(), lines: vec![], attached: vec![Item::Tool(call(ToolState::Running, 0))] }],
            ..Default::default()
        };
        assert!(tick_activity(&mut v, 1000));
        let Item::Tool(running) = &v.transcript[0] else { unreachable!() };
        assert_eq!(running.spin_ms, 1000);
        let Item::Tool(done) = &v.transcript[1] else { unreachable!() };
        assert_eq!(done.spin_ms, 7);
        let Item::Turn { attached, .. } = &v.transcript[2] else { unreachable!() };
        let Item::Tool(inside) = &attached[0] else { unreachable!() };
        assert_eq!(inside.spin_ms, 1000);
        v.generating = false;
        assert!(tick_activity(&mut v, 2000), "the activity goes");
        assert!(!tick_activity(&mut v, 3000), "an idle session draws nothing");
    }

    #[test]
    fn a_tool_of_the_harness_shows_no_json() {
        use serde_json::json;
        let note = |body: Option<ToolBody>| match body { Some(ToolBody::Note(text)) => text, other => panic!("{other:?}") };
        let m = |tool, value: Value| memory_words(harness_tool(tool), &value.to_string());
        let entry = |id: u64, body: &str| json!({"id": id, "category": "todo", "body": body});
        let Some(ToolBody::Output { lines, more, .. }) = m("mcp__harness__journal_search", json!({"entries": [entry(1, "first\nsecond"), entry(2, "other")]})) else { panic!() };
        assert_eq!((lines[0].as_str(), lines[1].as_str(), more), ("#1  todo  first", "#2  todo  other", 0));
        let many: Vec<Value> = (0..7).map(|i| entry(i, "x")).collect();
        let Some(ToolBody::Output { lines, more, .. }) = m("journal_list", json!({"total": 40, "entries": many})) else { panic!() };
        assert_eq!((lines.len(), more), (5, 35));
        assert_eq!(note(m("journal_search", json!({"entries": []}))), "Nothing found.");
        let Some(ToolBody::Output { label, .. }) = m("journal_search", json!({"entries": [entry(1, "x")], "linked_total": 7})) else { panic!() };
        assert_eq!(label.as_deref(), Some("7 linked"), "CON-118: the card counts the neighbours");
        assert_eq!(note(m("mcp__harness__journal_agenda", json!({"agenda": [{"flag": "todo", "total": 3}, {"flag": "question", "total": 1}]}))), "todo 3  ·  question 1");
        assert_eq!(note(m("mcp__harness__journal_close", json!({"closed": 7}))), "closed: 7");
        let Some(ToolBody::Output { label, lines, .. }) = m("mcp__harness__journal_branch", json!({"state": "open", "entries": [{"entry": entry(4, "the work")}]})) else { panic!() };
        assert_eq!((label.as_deref(), lines[0].as_str()), (Some("open"), "#4  todo  the work"));
        assert_eq!(harness_tool("mcp__other__journal_add"), "mcp__other__journal_add", "another server's is not ours");
        assert!(m("Bash", json!({"a": 1})).is_none());
    }

    #[test]
    fn a_journal_call_over_mcp_gets_the_journal_card() {
        let record = serde_json::json!({"id": 7, "created": "", "category": "decision", "tags": ["x"], "body": "why", "flags": []}).to_string();
        assert!(matches!(memory_card(harness_tool("mcp__harness__journal_add"), &record, "d"), Some(Item::Journal(_))));
        assert!(matches!(memory_card(harness_tool("mcp__harness__journal_get"), &record, "d"), Some(Item::Journal(_))));
    }

    #[test]
    fn reasoning_with_no_text_has_no_card() {
        let mut v = SessionView::default();
        append_thinking(&mut v, "10:00".into(), "", 1);
        append_thinking(&mut v, "10:00".into(), "  ", 2);
        assert!(v.transcript.is_empty(), "not even an empty turn");
        append_thinking(&mut v, "10:00".into(), "now words", 3);
        let Item::Turn { attached, .. } = &v.transcript[0] else { panic!() };
        let Item::Thinking(t) = &attached[0] else { panic!() };
        assert_eq!((t.lines[0].as_str(), t.secs, t.live), ("now words", 3, true));
    }

    #[test]
    fn background_tasks_keep_their_start_and_go_with_the_process() {
        let task = |id: &str| harness_api::event::BackgroundTask { id: id.into(), description: format!("job {id}"), kind: "local_bash".into() };
        let entry = |at_ms, event| LogEntry { seq: 0, at_ms, event };
        let mut feed = Feed::new(|_| String::new(), 1000);
        let mut v = SessionView::default();
        feed.apply(&mut v, &entry(100, Event::BackgroundTasks { tasks: vec![task("a")] }));
        feed.apply(&mut v, &entry(500, Event::BackgroundTasks { tasks: vec![task("a"), task("b")] }));
        assert_eq!((v.background[0].started_ms, v.background[1].started_ms), (100, 500));
        assert!(tick_activity(&mut v, 1000), "the panel moves on with nothing else going");
        assert!(!tick_activity(&mut v, 1001), "not within a frame");
        let cards = v.transcript.len();
        feed.apply(&mut v, &entry(550, Event::BackgroundDone { id: "foreground".into(), status: "completed".into(), summary: "Find a tool".into() }));
        assert_eq!(v.transcript.len(), cards, "a command in the foreground is no background task");
        feed.apply(&mut v, &entry(600, Event::BackgroundDone { id: "a".into(), status: "completed".into(), summary: "job a completed".into() }));
        assert!(matches!(v.transcript.last(), Some(Item::Inbound(m)) if m.text == "job a completed"));
        feed.apply(&mut v, &entry(700, Event::BackgroundTasks { tasks: vec![] }));
        assert!(v.background.is_empty());
        assert!(!tick_activity(&mut v, 5000), "an idle session draws nothing");
        feed.apply(&mut v, &entry(800, Event::BackgroundTasks { tasks: vec![task("c")] }));
        feed.replayed(&mut v);
        assert!(v.background.is_empty());
    }

    #[test]
    fn a_tab_in_output_keeps_its_columns() {
        assert_eq!(expand_tabs("bde8331\tqueued"), "bde8331 queued");
        assert_eq!(expand_tabs("a\tb\tc"), "a       b       c");
        assert_eq!(expand_tabs("12345678\tx"), "12345678        x");
        assert_eq!(expand_tabs("plain"), "plain");
        let ToolBody::Output { lines, .. } = output_body("a\tb") else { panic!() };
        assert_eq!(lines[0], "a       b");
    }
}

#[cfg(test)]
mod slave_questions {
    use super::*;

    #[test]
    fn a_slaves_question_opens_no_panel_for_the_user() {
        let ask = LogEntry { seq: 1, at_ms: 0, event: Event::ToolRequested { call: harness_api::ToolCall { id: "q".into(), name: "ask_user".into(), input: serde_json::json!({"question": "Which?"}) }, capabilities: vec![] } };
        let mut mine = SessionView::default();
        Feed::new(|_| String::new(), 1000).apply(&mut mine, &ask);
        assert!(mine.question.is_some(), "the user's own agent asks the user");
        let mut slave = SessionView { orchestra: Some("slave".into()), ..Default::default() };
        Feed::new(|_| String::new(), 1000).apply(&mut slave, &ask);
        assert!(slave.question.is_none(), "its orchestrator's to answer");
    }
}
