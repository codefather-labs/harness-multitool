//! Named `AppState` scenarios, one per reference mockup in `docs/design/concept`.
//! They are states of the real CLI, drawn by the real `view::draw`.

use crate::images::{Eyes, Icon};
use crate::model::{
    ActionKind, AppState, Brand, Composer, Diff, DiffLine, HeaderInfo, Inbound, InboundKind, Item, KeyHint, FlowchartEvent, MapCreated, MapTool, MindMapEvent,
    Mode, QuickAction, RecentSession, Role, SessionView, SplashPoint, SplashView, StartupPanel, ThinkingView, StartupView, StatusLine, Tab, TabIcon, TabMark,
    TipPart, Toast, Tone, ToolBody, ToolCall, ToolIcon, ToolState, Workspace,
};
use crate::debug::{sample_flowchart, sample_journal, sample_tool};

pub struct Fixture {
    pub name: &'static str,
    /// File stem of the mockup: `docs/design/concept/<mockup>-<theme>.png`.
    pub mockup: &'static str,
    pub build: fn() -> AppState,
}

pub const ALL: &[Fixture] = &[
    Fixture { name: "splash", mockup: "splash", build: splash },
    Fixture { name: "startup", mockup: "startup", build: startup },
    Fixture { name: "welcome", mockup: "welcome", build: welcome },
    Fixture { name: "main-session", mockup: "main-session", build: main_session },
    Fixture { name: "reasoning", mockup: "reasoning", build: reasoning },
    Fixture { name: "flow-instances", mockup: "tool-calls", build: flow_instances },
    Fixture { name: "settings-callbacks", mockup: "settings-callbacks", build: settings_callbacks },
    Fixture { name: "settings-auth", mockup: "settings-overlay", build: settings_auth },
    Fixture { name: "queued-messages", mockup: "main-session", build: queued_messages },
    Fixture { name: "question-options", mockup: "main-session", build: question_options },
    Fixture { name: "inbound-messages", mockup: "inbound-messages", build: inbound_messages },
    Fixture { name: "tool-calls", mockup: "tool-calls", build: tool_calls },
    Fixture { name: "journal-mindmap-events", mockup: "journal-mindmap-events", build: journal_mindmap_events },
    Fixture { name: "session-tabs", mockup: "session-tabs", build: session_tabs },
    Fixture { name: "maps", mockup: "journal-mindmap-events", build: maps },
    Fixture { name: "hooks", mockup: "tool-calls", build: hooks },
    Fixture { name: "permission", mockup: "permission", build: permission },
    Fixture { name: "settings-overlay", mockup: "settings-overlay", build: settings_overlay },
];

pub fn by_name(name: &str) -> Option<&'static Fixture> {
    ALL.iter().find(|f| f.name == name)
}

const VERSION: &str = "v0.1.0-dev";

fn header(location: &str) -> HeaderInfo {
    HeaderInfo {
        product: "harness-multitool".into(),
        version: VERSION.into(),
        location: location.into(),
        cwd: "~/work/harness-multitool".into(),
        branch: Some("main".into()),
        online: true,
    }
}

fn status(tokens: &str, cost: &str, budget: &str) -> StatusLine {
    StatusLine {
        fields: vec![
            ("ADAPTER".into(), "anthropic-api".into()),
            ("MODEL".into(), "claude-sonnet-4.5".into()),
            ("TOKENS".into(), tokens.into()),
            ("COST".into(), cost.into()),
            ("BUDGET".into(), budget.into()),
        ],
    }
}

fn session_hints() -> Vec<KeyHint> {
    vec![
        KeyHint::new("Enter", "send"),
        KeyHint::new("Shift+Enter", "newline"),
        KeyHint::new("@", "file"),
        KeyHint::new("/", "command"),
    ]
}

/// Three passes of one chart at once: closed rows, one opened, one pulsing.
pub fn flow_instances() -> AppState {
    let card = |instance: &str, node: &str, state: &str, expanded, pulse| {
        Item::FlowInstance(crate::model::FlowInstanceView {
            map: 1,
            instance: instance.into(),
            event: FlowchartEvent { id: 1, change: format!("{instance}: {node} {state}"), date: "14:52".into(), chart: crate::debug::sample_flowchart_down(), open: None, focus: None, pan: (0, 0) },
            expanded,
            pulse_until_ms: None,
            pulse,
        })
    };
    let view = SessionView {
        transcript: vec![turn(Role::User, "14:50", &["deploy all three"]), card("req-1", "n3", "entered", false, 0), card("req-2", "n2", "passed", true, 0), card("req-3", "n1", "entered", false, 6)],
        status: status("8.4k / 200k", "$0.06", "$10.00"),
        composer: Composer { text: String::new(), placeholder: "Type a message, / for commands, @ to mention...".into(), focused: true, hints: session_hints(), ..Default::default() },
        ..Default::default()
    };
    AppState { header: header("session/deploy"), eyes: Eyes::Open, pictures: false, mode: Mode::Session(ws(view, "deploy")) }
}

/// The settings on the Callbacks tab (settings-callbacks-*).
pub fn settings_callbacks() -> AppState {
    let mut s = crate::overlay::sample_settings();
    s.section = s.sections.iter().position(|n| n == "Callbacks").unwrap_or(0);
    s.focus = crate::model::SettingsPane::Page;
    s.callbacks = crate::overlay::sample_callbacks();
    s.callbacks.cursor = 2;
    s.callbacks.open = Some(2);
    with_overlay(crate::model::Overlay::Settings(s))
}

/// The agent asked, and waits for the answer.
pub fn question_options() -> AppState {
    use harness_api::question::{Question, QuestionOption};
    let mut state = main_session();
    if let Mode::Session(w) = &mut state.mode {
        let option = |label: &str, description: Option<&str>| QuestionOption { label: label.into(), description: description.map(str::to_string) };
        let v = w.view_mut();
        v.generating = false;
        v.activity = None;
        v.question = Some((
            Question { text: "What counts as a bad row?".into(), options: vec![option("Not a number", Some("negative values stay")), option("Not a number or negative", None), option("Any empty column", None)] },
            1,
        ));
    }
    state
}

/// A run under way, with messages waiting for it.
pub fn queued_messages() -> AppState {
    let mut state = main_session();
    if let Mode::Session(w) = &mut state.mode {
        w.view_mut().queued = vec!["stop before you touch the router".to_string().into(), "and add a test for the fallback".to_string().into()];
    }
    state
}

/// The settings on a signed-in plan.
pub fn settings_auth() -> AppState {
    let mut s = crate::overlay::sample_settings();
    s.focus = crate::model::SettingsPane::Page;
    s.provider = s.providers.iter().position(|p| p.name == crate::model::PLAN_PROVIDER).unwrap_or(0);
    s.cursor = s.providers.len();
    s.model = "gpt-5.5".into();
    s.auth = crate::overlay::sample_auth();
    with_overlay(crate::model::Overlay::Settings(s))
}

/// A model's reasoning in its three states (reasoning-*).
pub fn reasoning() -> AppState {
    let think = |lines: &[&str], live, secs, expanded| Item::Thinking(ThinkingView { lines: lines.iter().map(|s| s.to_string()).collect(), live, secs, expanded });
    let view = SessionView {
        transcript: vec![
            turn_with(
                Role::Assistant,
                "14:46",
                &["Found the issue: `mean()` was using integer division. I changed it to float division and verified that all tests pass."],
                vec![think(
                    &["1. Read the test to understand the expected behaviour.", "2. Inspect `calc.py` and check the implementation of `mean()`.", "3. Spotted integer division (`//`) instead of float division (`/`).", "4. Change it to float division and keep the function signature.", "5. Run the tests to verify the fix."],
                    false,
                    6,
                    true,
                )],
            ),
            turn_with(
                Role::Assistant,
                "14:49",
                &["The failing test was due to integer division in `mean()` returning a truncated value. I updated `calc.py` to use float division."],
                vec![
                    think(&["hidden until unfolded"], false, 38, false),
                    Item::Tool(crate::debug::tool(
                        "edit",
                        "calc.py",
                        ToolIcon::None,
                        ToolState::Done,
                        "applied",
                        "128 ms",
                        ToolBody::Diff(Diff {
                            hunk: "@@ -24 +24 @@".into(),
                            context: "def mean(values: list[int]) -> float:".into(),
                            lines: vec![DiffLine::Removed("    return sum(values) // len(values)".into()), DiffLine::Added("    return sum(values) / len(values)".into())],
                        }),
                    )),
                ],
            ),
            turn(Role::User, "14:52", &["fix the failing test in calc.py"]),
            turn_with(
                Role::Assistant,
                "14:52",
                &[],
                vec![think(
                    &["Let me open calc.py and the failing test to see the expected behaviour.", "Likely an edge case in the division handling (zero or type conversion).", "I'll check the test output, read the relevant code, and prepare a minimal fix.", "Then I'll apply the change and run the tests again to confirm."],
                    true,
                    12,
                    false,
                )],
            ),
        ],
        status: status("8.4k / 200k", "$0.06", "$10.00"),
        composer: Composer { text: String::new(), placeholder: "Type a message, / for commands, @ to mention...".into(), focused: true, hints: session_hints(), ..Default::default() },
        generating: true,
        activity: Some(crate::model::Activity { state: "Thinking".into(), started_ms: 0, now_ms: 12_000 }),
        ..Default::default()
    };
    AppState { header: header("session/calc"), eyes: Eyes::Open, pictures: false, mode: Mode::Session(ws(view, "calc")) }
}

pub fn welcome() -> AppState {
    AppState {
        header: header(VERSION),
        eyes: Eyes::Open,
        pictures: false,
        mode: Mode::Session(ws(SessionView {
            transcript: vec![
                Item::Welcome {
                    title: "Welcome to harness-multitool".into(),
                    tagline: "plugin-first coding harness · Rust + ratatui".into(),
                },
                Item::Tips {
                    label: "QUICK TIPS".into(),
                    lines: vec![
                        "Enter sends, Shift+Enter newline".into(),
                        "Ctrl+P command palette · / commands".into(),
                        "Drop files to attach · Ctrl+, settings".into(),
                    ],
                },
            ],
            status: status("0 / 200k", "$0.00", "$10.00"),
            composer: Composer {
                text: String::new(),
                placeholder: "Ask about the repo, request an edit, or type / for commands…".into(),
                focused: true,
                hints: session_hints(), ..Default::default() },
            ..Default::default()
        }, "4f7e3c2a")),
    }
}

fn turn(role: Role, time: &str, lines: &[&str]) -> Item {
    Item::Turn { role, time: time.into(), lines: lines.iter().map(|s| s.to_string()).collect(), attached: vec![] }
}

fn turn_with(role: Role, time: &str, lines: &[&str], attached: Vec<Item>) -> Item {
    Item::Turn { role, time: time.into(), lines: lines.iter().map(|s| s.to_string()).collect(), attached }
}

fn tool(name: &str, args: &str, outcome: &str, tone: Tone, duration: &str) -> ToolCall {
    ToolCall {
        name: name.into(),
        args: args.into(),
        args_strong: None,
        state: ToolState::Done,
        outcome: outcome.into(),
        outcome_tone: tone,
        diffstat: None,
        duration: duration.into(),
        expanded: false,
        body: ToolBody::None,
        icon: ToolIcon::None,
        spin_ms: 0,
    }
}

pub fn main_session() -> AppState {
    let edit = ToolCall {
        diffstat: Some((24, 3)),
        expanded: true,
        body: ToolBody::Diff(Diff {
            hunk: "@@ -18,6 +18,10 @@".into(),
            context: "pub fn select(&self, request: &Request)".into(),
            lines: vec![
                DiffLine::Removed("self.primary.clone()".into()),
                DiffLine::Added("self.adapters.iter().find(|a| a.supports(request))".into()),
                DiffLine::Added("    .or_else(|| self.fallback.as_ref())".into()),
            ],
        }),
        ..tool("edit_file", "src/router/mod.rs", "applied", Tone::Success, "")
    };
    let test = ToolCall {
        args_strong: Some("test".into()),
        ..tool("cargo", "-p harness-router", "12 passed", Tone::Success, "2.1 s")
    };
    AppState {
        header: header("session/4f7e3c2a"),
        eyes: Eyes::Open,
        pictures: false,
        mode: Mode::Session(ws(SessionView {
            transcript: vec![
                turn(Role::User, "14:32", &["Add a provider router with explicit model fallback."]),
                turn(Role::Assistant, "14:32", &["I will inspect the adapter registry, then add routing and tests."]),
                Item::Tool(tool("read_file", "src/providers/registry.rs", "completed", Tone::Success, "18 ms")),
                Item::Tool(tool("search", "\"trait ProviderAdapter\"", "3 matches", Tone::Neutral, "24 ms")),
                Item::Tool(edit),
                Item::Tool(test),
                turn(
                    Role::Assistant,
                    "14:33",
                    &[
                        "Routing is implemented. All 12 tests pass.",
                        "Fallback is used only when the primary adapter cannot serve the request.",
                        "Files changed: src/router/mod.rs, tests/router_fallback.rs",
                    ],
                ),
            ],
            status: status("18.2k / 200k", "$0.42", "$9.58"),
            composer: Composer {
                text: "Add a test for the no-adapter case and run the suite.".into(),
                placeholder: String::new(),
                focused: true,
                hints: session_hints(), ..Default::default() },
            ..Default::default()
        }, "4f7e3c2a")),
    }
}

fn brand() -> Brand {
    Brand {
        name: "harness-multitool".into(),
        version: VERSION.into(),
        slogan: "AI agent toolkit for development, automation and project knowledge".into(),
    }
}

pub fn splash() -> AppState {
    AppState {
        header: header(VERSION),
        eyes: Eyes::Open,
        pictures: false,
        mode: Mode::Splash(SplashView {
            brand: brand(),
            heading: "Welcome".into(),
            lines: vec!["A plugin-first agent harness you build to fit your work.".into()],
            points: splash_points(),
            button: "OK".into(),
            hint: "Enter to continue".into(),
        }),
    }
}

/// What the harness offers, short: shipped, specified, and - tagged - ideas and concepts
/// (docs/spec, docs/decisions; P-8: honest about what is not built).
pub fn splash_points() -> Vec<SplashPoint> {
    vec![
        SplashPoint::new("Every part is a Rust plugin you can replace", None),
        SplashPoint::new("Plugins hot-reload without restarting sessions", None),
        SplashPoint::new("Done means proven: tasks close with evidence", None),
        SplashPoint::new("Spec-first: specs and invariants checked", None),
        SplashPoint::new("Journal, mind maps, flowcharts as memory", None),
        SplashPoint::new("Callback daemon: custom callbacks and handlers", None),
        SplashPoint::new("Live flowcharts of running pipelines", None),
        SplashPoint::new("Step-by-step debug of distributed pipelines", None),
        SplashPoint::new("Plan mode: plans live in journal and maps", Some("idea")),
        SplashPoint::new("AI-first reliability monitoring, 24/7", Some("concept")),
        SplashPoint::new("Many agents in tabs, any model, any machine", None),
        SplashPoint::new("Linux, macOS, Windows, and in the browser", None),
    ]
}

fn recent(title: &str, path: &str, branch: &str, age: &str, model: &str) -> RecentSession {
    RecentSession { title: title.into(), path: path.into(), branch: branch.into(), age: age.into(), model: model.into() }
}

fn action(key: char, icon: Icon, title: &str, description: &str, kind: ActionKind) -> QuickAction {
    QuickAction { key, icon, title: title.into(), description: description.into(), kind }
}

fn tip(parts: &[(&str, bool)]) -> Vec<TipPart> {
    parts.iter().map(|(t, c)| TipPart { text: t.to_string(), code: *c }).collect()
}

pub fn startup() -> AppState {
    AppState {
        header: header(VERSION),
        eyes: Eyes::Open,
        pictures: false,
        mode: Mode::Startup(StartupView {
            brand: brand(),
            sessions: vec![
                recent("Router release (v0.1.0-rc.1)", "~/work/harness-multitool", "main", "2 min ago", "claude-sonnet-4.5"),
                recent("Metrics & dashboards", "~/work/tacticstrade.ai", "feat/metrics", "47 min ago", "claude-sonnet-4.5"),
                recent("Data collector (Binance)", "~/work/market-data", "collector-v2", "3 h ago", "claude-haiku-4.5"),
                recent("Android · Pocket Sampler", "~/work/pocket-sampler", "main", "6 h ago", "claude-sonnet-4.5"),
                recent("Bridge monitor dashboard", "~/work/bridge-monitor", "ui-refactor", "1 day ago", "gpt-4o-mini"),
            ],
            actions: vec![
                action('n', Icon::FilePlus, "New session", "Start a fresh session in this folder", ActionKind::NewSession),
                action('r', Icon::Play, "Resume session", "Continue a recent session", ActionKind::Resume),
                action('s', Icon::Gear, "Settings", "Providers, tools, permissions, theme", ActionKind::Settings),
                action('t', Icon::Map, "Start tour", "A quick guided introduction", ActionKind::Tour),
                action('?', Icon::Help, "Help", "Keybindings and usage reference", ActionKind::Help),
            ],
            focus: StartupPanel::Actions,
            selected_session: 0,
            selected_action: 0,
            status: StatusLine {
                fields: vec![
                    ("ADAPTER".into(), "anthropic-api".into()),
                    ("MODEL".into(), "claude-sonnet-4.5".into()),
                    ("THEME".into(), "dark".into()),
                ],
            },
            ready: "ready".into(),
            tips: vec![
                tip(&[("Use ", false), ("/journal", true), (" to add a project note, or ", false), ("/map", true), (" to create a mind map from the current context.", false)]),
                tip(&[("Press ", false), ("Ctrl+,", true), (" anywhere to open settings.", false)]),
                tip(&[("Drop files into the terminal to attach them to your message.", false)]),
            ],
            tip: 0,
        }),
    }
}

fn composer(placeholder: &str, hints: Vec<KeyHint>) -> Composer {
    Composer { text: String::new(), placeholder: placeholder.into(), focused: true, hints, ..Default::default() }
}

fn inbound(kind: InboundKind, from: &str, time: &str, text: &str) -> Item {
    Item::Inbound(Inbound { kind, from: from.into(), time: time.into(), text: text.into() })
}

fn read_file() -> ToolCall {
    tool("read_file", "src/metrics/mod.rs", "completed", Tone::Success, "18 ms")
}

/// inbound-messages-*: every channel's card, a toast that came in mid-generation.
pub fn inbound_messages() -> AppState {
    let view = SessionView {
        transcript: vec![
            turn(Role::User, "14:20", &["Prepare a release: run the build, update the changelog and create a Git tag."]),
            inbound(InboundKind::Telegram, "operator", "14:22", "Hey! Can you also include the new router metrics in this release?"),
            turn_with(
                Role::Assistant,
                "14:22",
                &["Sure! I'll build the project, update the changelog, add the metrics section, and create the tag..."],
                vec![Item::Tool(read_file())],
            ),
            inbound(
                InboundKind::Voice { duration: "0:27".into() },
                "operator",
                "14:24",
                "also make sure the metrics endpoint is enabled by default, and add a short note to the docs.",
            ),
            inbound(
                InboundKind::Agent { session: "session/9c1d8e7".into(), route: None },
                "reviewer-2",
                "14:25",
                "I've reviewed the changes in the router. LGTM. You can proceed with the release.",
            ),
            inbound(
                InboundKind::Callback { transport: "http".into(), status: "200 OK".into() },
                "ci:release",
                "14:26",
                "Build #9184 completed: success. Artifacts: dist/harness-multitool-linux-x86_64.tar.gz (49.2 MB).",
            ),
            inbound(InboundKind::Reminder { id: 42, every: "20m".into() }, "", "14:28", "Check the release board and update progress in the journal."),
            turn(
                Role::Assistant,
                "14:29",
                &[
                    "Build is green, changelog is updated, metrics are enabled by default, and the Git tag v0.1.0-rc.1 is created.",
                    "Release notes and docs have been updated. Anything else before I push the release?",
                ],
            ),
        ],
        status: status("24.7k / 200k", "$0.58", "$9.42"),
        composer: composer("Type a message, use / for commands, or drop files...", session_hints()),
        generating: true,
        activity: Some(crate::model::Activity { state: "Thinking".into(), started_ms: 0, now_ms: 12_000 }),
        ..Default::default()
    };
    let mut w = ws(view, "4f7e3c2a");
    w.toast = Some(Toast { kind: InboundKind::Telegram, from: "operator".into(), text: "Added a few notes about the metrics.".into() });
    AppState { header: header("session/4f7e3c2a"), mode: Mode::Session(w), eyes: Eyes::Open, pictures: false }
}

fn router_status() -> StatusLine {
    StatusLine {
        fields: vec![
            ("ADAPTER".into(), "anthropic-api".into()),
            ("MODEL".into(), "claude-sonnet-4.5".into()),
            ("TOKENS".into(), "12,840 (↑ 8,932 ↓ 3,908)".into()),
            ("DURATION".into(), "42.1 s".into()),
            ("SESSION".into(), "router".into()),
        ],
    }
}

/// tool-calls-*: the framed tool cards.
pub fn tool_calls() -> AppState {
    let tools: Vec<Item> = ["cat", "tg", "a2a", "journal", "fail", "run", "mcp-done", "mcp"].iter().filter_map(|k| sample_tool(k)).map(Item::Tool).collect();
    let view = SessionView {
        transcript: vec![
            turn(
                Role::User,
                "14:40",
                &["Run the checks, notify me in Telegram, send the patch to reviewer-2, write a journal entry and investigate the failing test."],
            ),
            turn_with(Role::Assistant, "14:40", &["On it. Running the required tools:"], tools),
        ],
        status: router_status(),
        composer: composer("Type a message, / for commands, @ to mention...", session_hints()),
        ..Default::default()
    };
    AppState { header: header("session/router"), mode: Mode::Session(ws(view, "router")), eyes: Eyes::Open, pictures: false }
}

/// journal-mindmap-events-*: a JOURNAL event, an empty map's creation, a drawn map.
pub fn journal_mindmap_events() -> AppState {
    let view = SessionView {
        transcript: vec![
            turn(
                Role::User,
                "14:27",
                &["Let's keep the decision and document it in the journal. Also create a mind map for the router fallback flow."],
            ),
            turn_with(
                Role::Assistant,
                "14:27",
                &["Done. I've recorded the decision in the journal and started a mind map for the fallback flow."],
                vec![
                    Item::Journal(sample_journal()),
                    Item::MapCreated(MapCreated { tool: MapTool::Flowchart, id: 3, title: "Router fallback".into(), shared: true }),
                ],
            ),
            turn_with(
                Role::Assistant,
                "14:28",
                &["Added the initial flow with the decision and fallback paths."],
                vec![Item::Flowchart(FlowchartEvent {
                    id: 3,
                    change: "+node n4, +arrow n2->n4 (ref #137)".into(),
                    date: "2026-10-28 14:28".into(),
                    chart: sample_flowchart(),
                    open: None,
                    pan: (0, 0),
                    focus: None,
                })],
            ),
            turn(Role::User, "14:28", &["Looks good. Let's continue with the implementation."]),
        ],
        status: router_status(),
        composer: composer("Type a message, / for commands, @ to mention...", session_hints()),
        ..Default::default()
    };
    AppState { header: header("session/router"), mode: Mode::Session(ws(view, "router")), eyes: Eyes::Open, pictures: false }
}

/// session-tabs-*: four sessions in one window, the a2a message between two of them.
pub fn session_tabs() -> AppState {
    let run = |name: &str, args: &str, dur: &str| Item::Tool(tool(name, args, "completed", Tone::Success, dur));
    let view = SessionView {
        transcript: vec![
            turn(Role::User, "14:20", &["Prepare a release: run the build, update the changelog and create a Git tag."]),
            turn_with(
                Role::Assistant,
                "14:22",
                &["Sure! I'll build the project, update the changelog, add the metrics section, and create the tag..."],
                vec![Item::Tool(read_file())],
            ),
            inbound(
                InboundKind::Agent { session: "session/reviewer-2".into(), route: Some("reviewer-2 ↔ router".into()) },
                "reviewer-2",
                "14:25",
                "I've reviewed the changes in the router. LGTM. You can proceed with the release.",
            ),
            turn_with(
                Role::Assistant,
                "14:29",
                &[
                    "Build is green, changelog is updated, metrics are enabled by default, and the Git tag v0.1.0-rc.1 is created.",
                    "Release notes and docs have been updated. Anything else before I push the release?",
                ],
                vec![run("run", "cargo build --release", "42.7 s"), run("git_tag", "v0.1.0-rc.1", "12 ms")],
            ),
            turn(Role::User, "14:31", &["No, that's all. Push it."]),
            turn_with(Role::Assistant, "14:31", &["Pushing to origin..."], vec![run("git_push", "origin main --tags", "3.8 s")]),
        ],
        status: status("24.7k / 200k", "$0.58", "$9.42"),
        composer: composer(
            "Type a message, use / for commands, or drop files...",
            vec![KeyHint::new("Ctrl+T", "new session"), KeyHint::new("Ctrl+W", "close session"), KeyHint::new("Alt+1..9", "switch session")],
        ),
        ..Default::default()
    };
    let other = |title: &str, icon, mark| Tab::new(title, icon, mark, SessionView { status: status("0 / 200k", "$0.00", "$10.00"), ..Default::default() });
    let w = Workspace { new_tab_runs_on: None,
        tabs: vec![
            Tab::new("router", TabIcon::Prompt, TabMark::None, view),
            other("reviewer-2", TabIcon::Agent, TabMark::Busy),
            other("docs", TabIcon::Doc, TabMark::Unread(2)),
            other("release", TabIcon::Lock, TabMark::Attention),
        ],
        active: 0,
        toast: None,
        reconnecting: None,
        moving: Vec::new(),
        overlay: None,
        drag: None,
        journal: crate::debug::known_entries(),
        live: false,
        outbox: Vec::new(),
        callbacks: None,
        telegram: None,
        voice: None,
        memory: None,
        journal_issues: None,
        profiles: None,
        auth: None,
    };
    AppState { header: header("session/router"), mode: Mode::Session(w), eyes: Eyes::Open, pictures: false }
}

/// Every map component at once: the flowchart gallery and the mind-map gallery.
pub fn maps() -> AppState {
    let view = SessionView {
        transcript: vec![turn_with(
            Role::Assistant,
            "01:20",
            &["Every block and component of both map types."],
            vec![
                Item::Flowchart(FlowchartEvent {
                    id: 4,
                    change: "every symbol and logic gate".into(),
                    date: "2026-11-01 01:20".into(),
                    chart: crate::debug::sample_flow_gallery(),
                    // The d1 node's two entries: #131 unfolded, #137 folded to its header.
                    open: Some(crate::model::MapElement::Node(2)),
                    pan: (0, 0),
                    focus: Some(131),
                }),
                Item::Flowchart(FlowchartEvent {
                    id: 3,
                    change: "the same flow top to bottom".into(),
                    date: "2026-11-01 01:20".into(),
                    chart: crate::debug::sample_flowchart_down(),
                    open: None,
                    pan: (0, 0),
                    focus: None,
                }),
                Item::MindMap(MindMapEvent {
                    id: 5,
                    change: "every node kind and status; flowcharts linked from n6 and a branch".into(),
                    date: "2026-11-01 01:20".into(),
                    map: crate::debug::sample_mind_gallery(),
                    // Folded: only one ticket is open in a transcript.
                    open: None,
                    pan: (0, 0),
                    focus: None,
                }),
            ],
        )],
        status: router_status(),
        composer: composer("Type a message, / for commands, @ to mention...", session_hints()),
        ..Default::default()
    };
    AppState { header: header("session/router"), mode: Mode::Session(ws(view, "router")), eyes: Eyes::Open, pictures: false }
}

/// Every hook outcome: session start and compaction with the two example scripts, a veto, a
/// failure, a timeout, a hook still running.
pub fn hooks() -> AppState {
    let mut transcript = Vec::new();
    for (k, time) in [("start", "14:20"), ("compact", "14:41"), ("block", "14:42"), ("fail", "14:43"), ("timeout", "14:44"), ("run", "14:45")] {
        if let Some(h) = crate::debug::sample_hook(k, time) {
            transcript.push(Item::Hook(h));
        }
    }
    let view = SessionView { transcript, status: router_status(), composer: composer("Type a message, / for commands, @ to mention...", session_hints()), ..Default::default() };
    AppState { header: header("session/router"), mode: Mode::Session(ws(view, "router")), eyes: Eyes::Open, pictures: false }
}

fn with_overlay(o: crate::model::Overlay) -> AppState {
    let mut s = main_session();
    if let Mode::Session(w) = &mut s.mode {
        w.overlay = Some(o);
    }
    s
}

/// permission-*: the shell call waiting for approval over the main session.
pub fn permission() -> AppState {
    with_overlay(crate::model::Overlay::Permission(crate::overlay::sample_permission()))
}

/// settings-overlay-*: the settings window on its provider page.
pub fn settings_overlay() -> AppState {
    with_overlay(crate::model::Overlay::Settings(crate::overlay::sample_settings()))
}

/// A session workspace with the journal entries the samples point at already fetched.
fn ws(view: SessionView, title: &str) -> Workspace {
    Workspace { journal: crate::debug::known_entries(), ..Workspace::one(view, title) }
}
