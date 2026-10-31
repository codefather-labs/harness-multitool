//! Named `AppState` scenarios, one per reference mockup in `docs/design/concept`.
//! They are states of the real CLI, drawn by the real `view::draw`.

use crate::model::{
    ActionKind, AppState, Brand, Composer, Diff, DiffLine, HeaderInfo, Item, KeyHint, Mode, QuickAction, RecentSession,
    Role, SessionView, SplashView, StartupPanel, StartupView, StatusLine, TipPart, Tone, ToolBody, ToolCall, ToolState,
};

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

pub fn welcome() -> AppState {
    AppState {
        header: header(VERSION),
        mode: Mode::Session(SessionView {
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
                hints: session_hints(),
            },
        }),
    }
}

fn turn(role: Role, time: &str, lines: &[&str]) -> Item {
    Item::Turn { role, time: time.into(), lines: lines.iter().map(|s| s.to_string()).collect() }
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
        mode: Mode::Session(SessionView {
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
                hints: session_hints(),
            },
        }),
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
        mode: Mode::Splash(SplashView {
            brand: brand(),
            heading: "Welcome".into(),
            lines: vec![
                "A plugin-first coding agent you build to fit your work.".into(),
                "Every part - providers, tools, policy, memory, frontends - is a plugin you can replace.".into(),
                "Done means proven: a task finishes only with its checks and evidence.".into(),
                "Your project journal and mind maps keep what matters across sessions.".into(),
            ],
            button: "OK".into(),
            hint: "Enter to continue".into(),
        }),
    }
}

fn recent(title: &str, path: &str, branch: &str, age: &str, model: &str) -> RecentSession {
    RecentSession { title: title.into(), path: path.into(), branch: branch.into(), age: age.into(), model: model.into() }
}

fn action(key: char, icon: &str, title: &str, description: &str, kind: ActionKind) -> QuickAction {
    QuickAction { key, icon: icon.into(), title: title.into(), description: description.into(), kind }
}

fn tip(parts: &[(&str, bool)]) -> Vec<TipPart> {
    parts.iter().map(|(t, c)| TipPart { text: t.to_string(), code: *c }).collect()
}

pub fn startup() -> AppState {
    AppState {
        header: header(VERSION),
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
                action('n', "✎", "New session", "Start a fresh session in this folder", ActionKind::NewSession),
                action('r', "▶", "Resume session", "Continue a recent session", ActionKind::Resume),
                action('s', "⚙", "Settings", "Providers, tools, permissions, theme", ActionKind::Settings),
                action('t', "▦", "Start tour", "A quick guided introduction", ActionKind::Tour),
                action('?', "?", "Help", "Keybindings and usage reference", ActionKind::Help),
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
