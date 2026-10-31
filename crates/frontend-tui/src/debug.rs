//! What the composer does with a sent line until the core is wired in: plain text becomes a
//! USER turn, and debug commands publish every other kind of transcript item, so each
//! component can be looked at in the live CLI (operator, 2026-11-01).
//!
//! `/help` lists the commands. The sample content mirrors the mockups in docs/design/concept.

use crate::model::{
    Diff, DiffLine, FlowEdge, FlowMap, FlowNode, Inbound, InboundKind, Item, JournalEvent, MapCreated, MapEvent, NodeShape, Role,
    SessionView, Tab, TabIcon, TabMark, Toast, Tone, ToolBody, ToolCall, ToolIcon, ToolState, Workspace,
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
    ("/journal", "journal entry event"),
    ("/map-new", "mind map created (empty)"),
    ("/map", "mind map updated, drawn"),
    ("/tool read|edit|cat|tg|a2a|journal|fail|run", "tool call"),
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

pub fn sample_map() -> FlowMap {
    let node = |id: &str, text: &str, shape, changed| FlowNode { id: id.into(), text: text.into(), shape, changed };
    let edge = |a: &str, b: &str, l: Option<&str>| FlowEdge { from: a.into(), to: b.into(), label: l.map(String::from) };
    FlowMap {
        nodes: vec![
            node("n1", "Start", NodeShape::Box, false),
            node("n2", "Primary OK?", NodeShape::Decision, false),
            node("n3", "Use primary model", NodeShape::Box, false),
            node("n4", "Use fallback (model / local)", NodeShape::Box, true),
            node("n5", "End", NodeShape::Box, false),
        ],
        edges: vec![
            edge("n1", "n2", None),
            edge("n2", "n3", Some("yes")),
            edge("n2", "n4", Some("no")),
            edge("n3", "n5", None),
            edge("n4", "n5", None),
        ],
    }
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
        "map-new" => attach(v, Item::MapCreated(MapCreated { id: 3, title: "Router fallback".into(), kind: "flowchart, shared".into() })),
        "map" => attach(
            v,
            Item::Map(MapEvent { id: 3, change: "+node n4, +edge n2->n4 (ref #137)".into(), date: "2026-10-28 14:28".into(), map: sample_map() }),
        ),
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
    w.tabs.push(Tab { title: title.into(), icon: TabIcon::Prompt, mark: TabMark::None, view });
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
    fn plain_text_becomes_a_user_turn() {
        let mut w = workspace();
        send(&mut w, "hello there");
        assert!(matches!(w.view().transcript.last(), Some(Item::Turn { role: Role::User, lines, .. }) if lines[0] == "hello there"));
        assert!(w.view().composer.text.is_empty());
    }

    #[test]
    fn every_debug_command_publishes_its_item() {
        let mut w = workspace();
        for cmd in ["/agent", "/tg", "/voice", "/a2a", "/callback", "/reminder", "/journal", "/map-new", "/map", "/help"] {
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
