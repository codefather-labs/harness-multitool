//! What the composer does with a sent line until the core is wired in: plain text becomes a
//! USER turn, and debug commands publish every other kind of transcript item, so each
//! component can be looked at in the live CLI (operator, 2026-11-01).
//!
//! `/help` lists the commands. The sample content mirrors the mockups in docs/design/concept.

use crate::model::{
    Arrow, Branch, Diff, DiffLine, FlowDirection, FlowNode, Flowchart, FlowchartEvent, HookEvent, HookOutcome, HookRun, Inbound, InboundKind,
    Item, JournalEvent, MapCreated, MapTool, MindKind, MindLink, MindMap, MindMapEvent, MindNode, NodeStatus, Outgoing, Role, ThinkingView, SessionView, Symbol, Tab,
    TabIcon, TabMark, Toast, Tone, ToolBody, ToolCall, ToolIcon, ToolState, Workspace,
};

/// The commands and what they publish, as `/help` shows them.
pub const COMMANDS: &[(&str, &str)] = &[
    ("/agent [text]", "assistant reply"),
    ("/busy", "toggle generating…"),
    ("/tg [text]", "Telegram message (+ toast)"),
    ("/voice [text]", "Telegram voice message"),
    ("/a2a [text]", "agent-to-agent message"),
    ("/callback [text]", "HTTP callback"),
    ("/reminder [text]", "journal reminder"),
    ("/reload", "rebuild, swap the binary, keep the sessions"),
    ("/init", "create docs/ and .harness/, overwriting nothing"),
    ("/stepdebug [off]", "step-debug mode, for large pipelines: each finished cycle is saved"),
    ("/plan [off]", "plan mode: the agent reads and plans, nothing can change"),
    ("/task [command|off|trust]", "the next message is a task: done only when its checks pass"),
    ("/journal", "journal entry event"),
    ("/thinking", "a model's reasoning, live"),
    ("/mindmap-new · /flowchart-new", "a mind map / flowchart created (empty)"),
    ("/mindmap", "a mind map with every node kind, status and flowchart link"),
    ("/flowchart [down]", "every flowchart symbol and gate / a flow top to bottom"),
    ("/permission", "a tool call waiting for approval"),
    ("/settings", "the settings window"),
    ("/tool read|edit|cat|tg|a2a|journal|fail|run", "tool call"),
    ("/hook start|compact|block|fail|timeout|run", "hooks firing on a loop event"),
    ("/hooks", "every hook outcome"),
    ("/toast [text]", "notification toast"),
    ("/tab [name]", "open a session tab"),
    ("/clear", "empty the transcript"),
    ("/exit, /quit", "close the harness (also Ctrl+C)"),
];

/// What sending the composer's line led to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sent {
    /// The line was empty.
    Nothing,
    /// A message or a command was handled.
    Done,
    /// `/exit` or `/quit`: the app closes.
    Quit,
}

/// Sends the active session's composer text.
pub fn submit(w: &mut Workspace) -> Sent {
    let text = std::mem::take(&mut w.view_mut().composer.text);
    let text = text.trim().to_string();
    if text.is_empty() {
        return Sent::Nothing;
    }
    if matches!(text.as_str(), "/exit" | "/quit" | "/q") {
        return Sent::Quit;
    }
    w.view_mut().scroll = 0;
    if let Some(cmd) = text.strip_prefix('/') {
        command(w, cmd);
    } else if w.live {
        // A running session draws the message from its log, like everything else.
        let tab = w.tabs[w.active].id;
        match w.view_mut().task.take() {
            Some(steps) => w.outbox.push(Outgoing::Task { tab, text, steps }),
            None => w.outbox.push(Outgoing::Prompt { tab, text }),
        }
    } else {
        let v = w.view_mut();
        let time = now(v);
        v.transcript.push(Item::Turn { role: Role::User, time, lines: vec![text], attached: vec![] });
    }
    Sent::Done
}

/// The time stamp of the next message (`HH:MM`); every message moves the clock a minute on.
fn now(v: &mut SessionView) -> String {
    if v.clock == 0 {
        v.clock = 14 * 60 + 20;
    }
    let s = format!("{:02}:{:02}", v.clock / 60 % 24, v.clock % 60);
    v.clock += 1;
    s
}

fn or<'a>(arg: &'a str, default: &'a str) -> &'a str {
    if arg.trim().is_empty() { default } else { arg.trim() }
}

/// Appends `it` to the newest assistant turn when that turn is the last item, else starts a
/// new assistant turn for it: what the agent does belongs to the reply that does it.
fn attach(v: &mut SessionView, it: Item) {
    if let Some(Item::Turn { role: Role::Assistant, attached, .. }) = v.transcript.last_mut() {
        attached.push(it);
        return;
    }
    let time = now(v);
    v.transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec!["On it.".into()], attached: vec![it] });
}

fn inbound(v: &mut SessionView, kind: InboundKind, from: &str, text: &str) {
    let time = now(v);
    v.transcript.push(Item::Inbound(Inbound { kind, from: from.into(), time, text: text.into() }));
}

pub fn tool(name: &str, args: &str, icon: ToolIcon, state: ToolState, outcome: &str, duration: &str, body: ToolBody) -> ToolCall {
    ToolCall {
        name: name.into(),
        args: args.into(),
        args_strong: None,
        state,
        outcome: outcome.into(),
        outcome_tone: if state == ToolState::Failed { Tone::Error } else { Tone::Success },
        diffstat: None,
        duration: duration.into(),
        expanded: !matches!(body, ToolBody::None),
        body,
        icon,
    }
}

pub fn sample_tool(kind: &str) -> Option<ToolCall> {
    let out = |label: Option<&str>, lines: &[&str], numbered: bool, more: usize| ToolBody::Output {
        label: label.map(String::from),
        lines: lines.iter().map(|s| s.to_string()).collect(),
        numbered,
        more,
    };
    Some(match kind {
        "read" => tool("read_file", "src/metrics/mod.rs", ToolIcon::None, ToolState::Done, "completed", "18 ms", ToolBody::None),
        "edit" => ToolCall {
            diffstat: Some((24, 3)),
            ..tool(
                "edit_file",
                "src/router/mod.rs",
                ToolIcon::None,
                ToolState::Done,
                "applied",
                "",
                ToolBody::Diff(Diff {
                    hunk: "@@ -18,6 +18,10 @@".into(),
                    context: "pub fn select(&self, request: &Request)".into(),
                    lines: vec![
                        DiffLine::Removed("self.primary.clone()".into()),
                        DiffLine::Added("self.adapters.iter().find(|a| a.supports(request))".into()),
                        DiffLine::Added("    .or_else(|| self.fallback.as_ref())".into()),
                    ],
                }),
            )
        },
        "cat" => tool(
            "shell",
            "cat src/router/mod.rs",
            ToolIcon::Shell,
            ToolState::Done,
            "completed",
            "328 ms",
            out(
                None,
                &[
                    "use std::sync::Arc;",
                    "use anyhow::Result;",
                    "use axum::{Router, routing::{get, post}};",
                    "use crate::fallback::Fallback;",
                    "",
                    "pub struct RouterState {",
                    "    pub fallback: Arc<Fallback>,",
                    "}",
                ],
                true,
                38,
            ),
        ),
        "tg" => tool(
            "telegram_send",
            "chat: @operator   topic: ops   message: reply",
            ToolIcon::Send,
            ToolState::Done,
            "delivered",
            "412 ms",
            ToolBody::Bubble("Router update is ready: fallback added, tests are green (47/47). Pushed to main.".into()),
        ),
        "a2a" => tool(
            "agent_send",
            "to: reviewer-2   (a2a)",
            ToolIcon::Agent,
            ToolState::Done,
            "completed",
            "221 ms",
            ToolBody::Note("Please review the router patch (fallback) and let me know if any changes are needed.".into()),
        ),
        "journal" => tool(
            "journal_add",
            "id: #137   title: decision   tags: router, fallback",
            ToolIcon::Journal,
            ToolState::Done,
            "completed",
            "87 ms",
            ToolBody::Note("Wrote journal entry #137 “decision”.".into()),
        ),
        "fail" => tool(
            "shell",
            "cargo test -p router -- --nocapture",
            ToolIcon::Shell,
            ToolState::Failed,
            "failed (exit 101)",
            "4.6 s",
            out(
                Some("stderr (tail)"),
                &["error[E0599]: no method named `route_with_fallback` found for struct `Router`", "  --> src/router/mod.rs:121:18"],
                false,
                0,
            ),
        ),
        "run" => tool(
            "shell",
            "cargo test -p router --test fallback_flow",
            ToolIcon::Shell,
            ToolState::Running,
            "running",
            "12.4 s",
            out(Some("output (live)"), &["running 3 tests", "test fallback::uses_local_on_timeout ... ok", "test fallback::retries_then_falls_back ..."], false, 0),
        ),
        _ => return None,
    })
}

fn fnode(id: &str, text: &str, symbol: Symbol) -> FlowNode {
    FlowNode { id: id.into(), text: text.into(), symbol, refs: vec![], changed: false, active: false }
}

fn arrow(a: &str, b: &str, label: Option<&str>) -> Arrow {
    Arrow { from: a.into(), to: b.into(), label: label.map(String::from), refs: vec![] }
}

/// The journal entries the samples point at, as the session would have fetched them.
pub fn known_entries() -> Vec<JournalEvent> {
    vec![
        sample_journal(),
        JournalEvent {
            id: 131,
            category: "finding".into(),
            tags: vec!["router".into(), "metrics".into()],
            date: "2026-10-27 18:02".into(),
            text: "The primary adapter fails about 2% of requests at peak hours (metrics 2026-10-27, 14:00-16:00); every failure is a timeout, none is a 5xx.".into(),
        },
    ]
}

/// The router fallback flowchart of journal-mindmap-events-*.
pub fn sample_flowchart() -> Flowchart {
    let mut n4 = fnode("n4", "Use fallback (model / local)", Symbol::Process);
    n4.changed = true;
    n4.refs = vec![137];
    Flowchart {
        direction: FlowDirection::Right,
        nodes: vec![
            fnode("n1", "Start", Symbol::Start),
            fnode("n2", "Primary OK?", Symbol::Decision),
            fnode("n3", "Use primary model", Symbol::Process),
            n4,
            fnode("n5", "End", Symbol::End),
        ],
        arrows: vec![
            arrow("n1", "n2", None),
            arrow("n2", "n3", Some("yes")),
            arrow("n2", "n4", Some("no")),
            arrow("n3", "n5", None),
            arrow("n4", "n5", None),
        ],
    }
}

/// The same flow, top to bottom (the backend's default direction).
pub fn sample_flowchart_down() -> Flowchart {
    Flowchart { direction: FlowDirection::Down, ..sample_flowchart() }
}

/// Every flowchart symbol and every logic gate, in four short chains.
pub fn sample_flow_gallery() -> Flowchart {
    use crate::model::Gate;
    let mut changed = fnode("p3", "Changed node", Symbol::Process);
    changed.changed = true;
    Flowchart {
        direction: FlowDirection::Right,
        nodes: vec![
            fnode("s1", "Start", Symbol::Start),
            fnode("i1", "Read input", Symbol::Io),
            FlowNode { refs: vec![137, 131], ..fnode("d1", "Valid?", Symbol::Decision) },
            fnode("p1", "Process", Symbol::Process),
            fnode("e1", "End", Symbol::End),
            fnode("m1", "Manual fix", Symbol::Manual),
            fnode("r1", "Prepare", Symbol::Prep),
            // The node a live run is at: outlined.
            FlowNode { active: true, ..fnode("u1", "Subprocess", Symbol::Subprocess) },
            fnode("o1", "Report", Symbol::Document),
            fnode("b1", "Store", Symbol::Data),
            fnode("w1", "Wait 5 s", Symbol::Delay),
            fnode("c1", "A", Symbol::Connector),
            fnode("g1", "", Symbol::Gate(Gate::And)),
            fnode("g2", "", Symbol::Gate(Gate::Or)),
            fnode("g3", "", Symbol::Gate(Gate::Xor)),
            fnode("g4", "", Symbol::Gate(Gate::Not)),
            fnode("c2", "B", Symbol::Connector),
            fnode("g5", "", Symbol::Gate(Gate::Nand)),
            fnode("g6", "", Symbol::Gate(Gate::Nor)),
            fnode("g7", "", Symbol::Gate(Gate::Xnor)),
            changed,
            fnode("t1", "A note on the flow", Symbol::Note),
        ],
        arrows: vec![
            arrow("s1", "i1", None),
            arrow("i1", "d1", None),
            arrow("d1", "p1", Some("yes")),
            Arrow { refs: vec![131], ..arrow("d1", "m1", Some("no")) },
            arrow("p1", "e1", None),
            arrow("r1", "u1", None),
            arrow("u1", "o1", None),
            arrow("o1", "b1", None),
            arrow("b1", "w1", None),
            arrow("c1", "g1", None),
            arrow("g1", "g2", None),
            arrow("g2", "g3", None),
            arrow("g3", "g4", None),
            arrow("c2", "g5", None),
            arrow("g5", "g6", None),
            arrow("g6", "g7", None),
            arrow("g7", "p3", None),
        ],
    }
}

/// Every mind-map node kind and status, journal refs, a cross link, and flowcharts linked
/// from a node, a branch and a link.
pub fn sample_mind_gallery() -> MindMap {
    let with = |id: &str, text: &str, kind: MindKind, status: NodeStatus, refs: &[u32]| MindNode {
        id: id.into(),
        text: text.into(),
        kind,
        status,
        refs: refs.to_vec(),
        flows: vec![],
        changed: false,
    };
    let mut changed = with("n13", "Changed in this event", MindKind::Fact, NodeStatus::Open, &[]);
    changed.changed = true;
    let mut chain = with("n6", "Use an explicit chain", MindKind::Decision, NodeStatus::Open, &[137]);
    chain.flows = vec![3];
    let mut branches: Vec<Branch> = [
        ("n1", "n2"),
        ("n1", "n3"),
        ("n1", "n4"),
        ("n1", "n5"),
        ("n2", "n6"),
        ("n2", "n7"),
        ("n3", "n8"),
        ("n3", "n9"),
        ("n4", "n10"),
        ("n4", "n11"),
        ("n5", "n12"),
        ("n5", "n13"),
        ("n5", "n14"),
    ]
    .iter()
    .map(|(a, b)| Branch { from: a.to_string(), to: b.to_string(), flows: vec![] })
    .collect();
    branches[11].flows = vec![4];
    MindMap {
        nodes: vec![
            with("n1", "Router fallback", MindKind::Topic, NodeStatus::Open, &[]),
            with("n2", "Never drop a request", MindKind::Goal, NodeStatus::Open, &[]),
            with("n3", "Findings", MindKind::Fact, NodeStatus::Open, &[]),
            with("n4", "Options", MindKind::Idea, NodeStatus::Open, &[]),
            with("n5", "Work", MindKind::Task, NodeStatus::Open, &[]),
            chain,
            with("n7", "Which timeout for local?", MindKind::Question, NodeStatus::Open, &[]),
            with("n8", "Primary fails ~2% at peak", MindKind::Fact, NodeStatus::Open, &[131]),
            with("n9", "Retries alone are enough", MindKind::Hypothesis, NodeStatus::Open, &[]),
            with("n10", "Retry with backoff only", MindKind::DeadEnd, NodeStatus::Dropped, &[]),
            with("n11", "See the provider docs", MindKind::Note, NodeStatus::Open, &[]),
            with("n12", "Write the router tests", MindKind::Task, NodeStatus::Done, &[]),
            changed,
            with("n14", "Measure fallback latency", MindKind::Task, NodeStatus::Open, &[]),
        ],
        branches,
        links: vec![MindLink { from: "n9".into(), to: "n6".into(), label: "contradicts".into(), flows: vec![] }],
    }
}

/// Hooks on a loop event: `start` and `compact` run the two example scripts, the others show
/// the remaining outcomes.
pub fn sample_hook(kind: &str, time: &str) -> Option<HookEvent> {
    let run = |script: &str, outcome: HookOutcome, duration: &str| HookRun { script: script.into(), outcome, duration: duration.into() };
    let fact = "fact-check-cognitive-protocol.sh";
    let lookup = "harness-lookup.sh";
    let ev = |event: &str, subject: Option<&str>, runs: Vec<HookRun>| HookEvent {
        event: event.into(),
        subject: subject.map(String::from),
        time: time.into(),
        runs,
    };
    Some(match kind {
        "start" => ev(
            "SessionStart",
            None,
            vec![
                run(fact, HookOutcome::Context(vec!["[self-check] run the three cognitive self-check protocols: inbound, facts, decisions".into()]), "38 ms"),
                run(
                    lookup,
                    HookOutcome::Context(vec!["project: harness-multitool · journal: 234 entries · shared map #1".into(), "open: 6 todos, 3 questions".into()]),
                    "112 ms",
                ),
            ],
        ),
        // After compaction: the hooks put back what the summary must not lose.
        "compact" => ev(
            "PostCompact",
            Some("context 182k → 41k"),
            vec![
                run(fact, HookOutcome::Context(vec!["[self-check] run the three cognitive self-check protocols: inbound, facts, decisions".into()]), "31 ms"),
                run(lookup, HookOutcome::Context(vec!["restored: 12 specifications, 4 constraints, open nodes of map #1".into()]), "96 ms"),
            ],
        ),
        "block" => ev(
            "PreToolUse",
            Some("shell · git push --force origin main"),
            vec![run("guard-force-push.sh", HookOutcome::Blocked("force-push to main is not allowed; push a branch and open a PR".into()), "14 ms")],
        ),
        "fail" => ev(
            "PostToolUse",
            Some("edit_file · src/router/mod.rs"),
            vec![run("format-on-save.sh", HookOutcome::Failed { code: 1, stderr: vec!["rustfmt: error: expected one of `,` or `}` at src/router/mod.rs:121".into()] }, "220 ms")],
        ),
        "timeout" => ev("Stop", None, vec![run("notify-telegram.sh", HookOutcome::Timeout, "10.0 s")]),
        "run" => ev("UserPromptSubmit", None, vec![run(fact, HookOutcome::Ok, "29 ms"), run("index-attachments.sh", HookOutcome::Running, "1.4 s")]),
        _ => return None,
    })
}

pub fn sample_journal() -> JournalEvent {
    JournalEvent {
        id: 137,
        category: "decision".into(),
        tags: vec!["router".into(), "fallback".into()],
        date: "2026-10-28 14:27".into(),
        text: "Use explicit fallback chain for the router: primary → fallback model → local (on error or timeout).".into(),
    }
}

/// Runs a debug command (the text after `/`).
fn command(w: &mut Workspace, cmd: &str) {
    let (name, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let v = w.view_mut();
    match name {
        "help" | "?" => {
            v.transcript.push(Item::Tips {
                label: "DEBUG COMMANDS".into(),
                lines: COMMANDS.iter().map(|(c, d)| format!("{c:<44} {d}")).chain(std::iter::once("plain text: your message".to_string())).collect(),
            });
        }
        "init" => {
            if w.live {
                let tab = w.tabs[w.active].id;
                w.outbox.push(Outgoing::Init { tab });
            } else {
                w.view_mut().transcript.push(Item::Tips { label: "INIT".into(), lines: vec!["Only a live session can initialize a project.".into()] });
            }
        }
        "stepdebug" => {
            let on = arg.trim() != "off";
            if w.live {
                let tab = w.tabs[w.active].id;
                w.outbox.push(Outgoing::StepDebug { tab, on });
            } else {
                w.view_mut().transcript.push(Item::Tips { label: "STEP DEBUG".into(), lines: vec!["Only a live session has a step-debug mode.".into()] });
            }
        }
        "plan" => {
            let on = arg.trim() != "off";
            if w.live {
                let tab = w.tabs[w.active].id;
                w.outbox.push(Outgoing::Plan { tab, on });
            } else {
                w.view_mut().transcript.push(Item::Tips { label: "PLAN MODE".into(), lines: vec!["Only a live session has a plan mode.".into()] });
            }
        }
        "task" => {
            let arg = arg.trim();
            if arg == "trust" {
                let tab = w.tabs[w.active].id;
                w.outbox.push(Outgoing::ApproveChecks { tab });
            } else if arg == "off" {
                v.task = None;
                v.transcript.push(Item::Tips { label: "TASK".into(), lines: vec!["The next message is an ordinary one.".into()] });
            } else {
                let steps = v.task.get_or_insert_with(Vec::new);
                if !arg.is_empty() {
                    steps.push(arg.to_string());
                }
                let mut lines = vec!["The next message is a task. It is done only when these pass, then .harness/acceptance.toml:".to_string()];
                lines.extend(steps.iter().map(|s| format!("$ {s}")));
                lines.push("Once done, the tab's session is over: open a new tab to go on. /task off cancels.".to_string());
                v.transcript.push(Item::Tips { label: "TASK".into(), lines });
            }
        }
        "reload" => {
            if w.live {
                w.outbox.push(Outgoing::Reload);
            } else {
                w.view_mut().transcript.push(Item::Tips { label: "RELOAD".into(), lines: vec!["Only a live session can reload.".into()] });
            }
        }
        "thinking" => {
            let live = Item::Thinking(ThinkingView { lines: vec!["Let me open the file and see what the test expects.".into(), "Likely an edge case in the division.".into()], live: true, secs: 12, expanded: false });
            let time = now(w.view_mut());
            w.view_mut().transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec![], attached: vec![live] });
        }
        "agent" | "a" => {
            let time = now(v);
            let text = or(arg, "Sure! I'll build the project, update the changelog, add the metrics section, and create the tag.");
            v.transcript.push(Item::Turn { role: Role::Assistant, time, lines: vec![text.into()], attached: vec![] });
        }
        "busy" => v.generating = !v.generating,
        "tg" | "telegram" => {
            let text = or(arg, "Hey! Can you also include the new router metrics in this release?");
            inbound(v, InboundKind::Telegram, "operator", text);
            w.toast = Some(Toast { kind: InboundKind::Telegram, from: "operator".into(), text: text.into() });
        }
        "voice" => inbound(
            v,
            InboundKind::Voice { duration: "0:27".into() },
            "operator",
            or(arg, "also make sure the metrics endpoint is enabled by default, and add a short note to the docs."),
        ),
        "a2a" | "agent-msg" => {
            let peer = w.tabs.iter().enumerate().find(|(i, _)| *i != w.active).map(|(_, t)| t.title.clone());
            let me = w.tabs[w.active].title.clone();
            let v = w.view_mut();
            let route = peer.as_ref().map(|p| format!("{p} ↔ {me}"));
            inbound(
                v,
                InboundKind::Agent { session: format!("session/{}", peer.as_deref().unwrap_or("9c1d8e7")), route },
                peer.as_deref().unwrap_or("reviewer-2"),
                or(arg, "I've reviewed the changes in the router. LGTM. You can proceed with the release."),
            );
        }
        "callback" | "cb" => inbound(
            v,
            InboundKind::Callback { transport: "http".into(), status: "200 OK".into() },
            "ci:release",
            or(arg, "Build #9184 completed: success. Artifacts: dist/harness-multitool-linux-x86_64.tar.gz (49.2 MB)."),
        ),
        "reminder" => inbound(
            v,
            InboundKind::Reminder { id: 42, every: "20m".into() },
            "",
            or(arg, "Check the release board and update progress in the journal."),
        ),
        "journal" => attach(v, Item::Journal(sample_journal())),
        "mindmap-new" => attach(v, Item::MapCreated(MapCreated { tool: MapTool::MindMap, id: 2, title: "Router fallback".into(), shared: true })),
        "flowchart-new" | "map-new" => {
            attach(v, Item::MapCreated(MapCreated { tool: MapTool::Flowchart, id: 3, title: "Router fallback".into(), shared: true }))
        }
        "mindmap" | "mind" => attach(
            v,
            Item::MindMap(MindMapEvent { id: 5, change: "every node kind and status; flowcharts linked from n6 and a branch".into(), date: "2026-11-01 01:20".into(), map: sample_mind_gallery(), open: None, pan: (0, 0), focus: None }),
        ),
        "flowchart" | "flow" | "map" => {
            let (change, chart) = if arg.trim() == "down" {
                ("+node n4, +arrow n2->n4 (ref #137), top to bottom", sample_flowchart_down())
            } else {
                ("every symbol and logic gate", sample_flow_gallery())
            };
            attach(v, Item::Flowchart(FlowchartEvent { id: 4, change: change.into(), date: "2026-11-01 01:20".into(), chart, open: None, pan: (0, 0), focus: None }))
        }
        "permission" => w.overlay = Some(crate::model::Overlay::Permission(crate::overlay::sample_permission())),
        "settings" => w.overlay = Some(crate::model::Overlay::Settings(crate::overlay::settings_for(w))),
        "tool" | "t" => match sample_tool(or(arg, "read")) {
            Some(c) => attach(v, Item::Tool(c)),
            None => v.transcript.push(Item::Tips { label: "UNKNOWN TOOL".into(), lines: vec!["/tool read|edit|cat|tg|a2a|journal|fail|run".into()] }),
        },
        "toast" => {
            w.toast = Some(Toast { kind: InboundKind::Telegram, from: "operator".into(), text: or(arg, "Added a few notes about the metrics.").into() })
        }
        "tab" => {
            let title = or(arg, &format!("session-{}", w.tabs.len() + 1)).to_string();
            open_tab(w, &title);
        }
        "clear" => v.transcript.clear(),
        _ => v.transcript.push(Item::Tips { label: "UNKNOWN COMMAND".into(), lines: vec![format!("/{name} - /help lists the commands")] }),
    }
}

/// Opens a new session tab after the others and switches to it; the new session starts with
/// the welcome card and the composer of the current one.
pub fn open_tab(w: &mut Workspace, title: &str) {
    let mut view = w.view().clone();
    view.transcript = vec![Item::Welcome { title: format!("New session: {title}"), tagline: "plugin-first coding harness · Rust + ratatui".into() }];
    view.composer.text.clear();
    view.scroll = 0;
    view.generating = false;
    // A new session has spent nothing yet.
    for (key, value) in &mut view.status.fields {
        match key.as_str() {
            "TOKENS" => *value = "0".into(),
            "COST" => *value = "$0.00".into(),
            _ => {}
        }
    }
    w.tabs.push(Tab::new(title, TabIcon::Prompt, TabMark::None, view));
    w.active = w.tabs.len() - 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::Mode;

    fn workspace() -> Workspace {
        match fixtures::welcome().mode {
            Mode::Session(w) => w,
            _ => unreachable!(),
        }
    }

    fn send(w: &mut Workspace, text: &str) {
        w.view_mut().composer.text = text.into();
        assert_eq!(submit(w), Sent::Done);
    }

    #[test]
    fn plan_switches_a_live_session() {
        let mut w = workspace();
        send(&mut w, "/plan");
        assert!(w.outbox.is_empty(), "the fixture has no session");
        w.live = true;
        send(&mut w, "/plan");
        send(&mut w, "/plan off");
        assert!(matches!(&w.outbox[..], [Outgoing::Plan { on: true, .. }, Outgoing::Plan { on: false, .. }]), "{:?}", w.outbox);
    }

    #[test]
    fn task_arms_the_next_live_message_only() {
        let mut w = workspace();
        w.live = true;
        send(&mut w, "/task cargo test");
        send(&mut w, "/task cargo clippy");
        send(&mut w, "fix it");
        send(&mut w, "and a question");
        let out = std::mem::take(&mut w.outbox);
        assert!(matches!(&out[0], Outgoing::Task { text, steps, .. } if text == "fix it" && steps == &["cargo test", "cargo clippy"]), "{out:?}");
        assert!(matches!(&out[1], Outgoing::Prompt { text, .. } if text == "and a question"), "{out:?}");

        send(&mut w, "/task cargo test");
        send(&mut w, "/task off");
        send(&mut w, "plain");
        assert!(matches!(&w.outbox[0], Outgoing::Prompt { .. }), "{:?}", w.outbox);
        send(&mut w, "/task");
        assert_eq!(w.view().task, Some(vec![]), "the acceptance checks alone");
        send(&mut w, "/task trust");
        assert!(matches!(w.outbox.last(), Some(Outgoing::ApproveChecks { .. })), "{:?}", w.outbox);
        assert_eq!(w.view().task, Some(vec![]), "approving does not add a step");
    }

    #[test]
    fn plain_text_becomes_a_user_turn() {
        let mut w = workspace();
        send(&mut w, "hello there");
        assert!(matches!(w.view().transcript.last(), Some(Item::Turn { role: Role::User, lines, .. }) if lines[0] == "hello there"));
        assert!(w.view().composer.text.is_empty());
    }

    #[test]
    fn every_debug_command_publishes_its_item() {
        let mut w = workspace();
        for cmd in ["/agent", "/tg", "/voice", "/a2a", "/callback", "/reminder", "/journal", "/mindmap-new", "/flowchart-new", "/mindmap", "/flowchart", "/flowchart down", "/hook", "/hooks", "/help"] {
            let before = w.view().transcript.len();
            let attached_before: usize = w.view().transcript.iter().map(|i| if let Item::Turn { attached, .. } = i { attached.len() } else { 0 }).sum();
            send(&mut w, cmd);
            let attached_after: usize = w.view().transcript.iter().map(|i| if let Item::Turn { attached, .. } = i { attached.len() } else { 0 }).sum();
            assert!(w.view().transcript.len() > before || attached_after > attached_before, "{cmd} published nothing");
        }
        for kind in ["read", "edit", "cat", "tg", "a2a", "journal", "fail", "run"] {
            assert!(sample_tool(kind).is_some(), "{kind}");
        }
        assert!(w.toast.is_some(), "/tg shows a toast");
    }

    #[test]
    fn exit_and_quit_close_the_harness() {
        let mut w = workspace();
        for cmd in ["/exit", "/quit", " /exit "] {
            w.view_mut().composer.text = cmd.into();
            assert_eq!(submit(&mut w), Sent::Quit, "{cmd:?}");
        }
        w.view_mut().composer.text = String::new();
        assert_eq!(submit(&mut w), Sent::Nothing);
    }

    #[test]
    fn tabs_open_and_keep_their_own_transcript() {
        let mut w = workspace();
        send(&mut w, "first tab");
        send(&mut w, "/tab review");
        assert_eq!((w.tabs.len(), w.active), (2, 1));
        assert!(!w.view().transcript.iter().any(|i| matches!(i, Item::Turn { lines, .. } if lines[0] == "first tab")));
        w.active = 0;
        assert!(w.view().transcript.iter().any(|i| matches!(i, Item::Turn { lines, .. } if lines[0] == "first tab")));
    }
}
