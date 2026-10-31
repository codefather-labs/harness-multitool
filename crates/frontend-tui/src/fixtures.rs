//! Named `AppState` scenarios, one per reference mockup in `docs/design/concept`.
//! They are states of the real CLI, drawn by the real `view::draw`.

use crate::model::{AppState, Composer, HeaderInfo, Item, KeyHint, Mode, SessionView};

pub struct Fixture {
    pub name: &'static str,
    /// File stem of the mockup: `docs/design/concept/<mockup>-<theme>.png`.
    pub mockup: &'static str,
    pub build: fn() -> AppState,
}

pub const ALL: &[Fixture] = &[Fixture { name: "welcome", mockup: "welcome", build: welcome }];

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
        adapter_model: Some("anthropic-api / claude-sonnet-4.5".into()),
        branch: Some("main".into()),
        online: true,
    }
}

fn welcome() -> AppState {
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
            composer: Composer {
                text: String::new(),
                placeholder: "Ask about the repo, request an edit, or type / for commands…".into(),
                focused: true,
            },
            hints: vec![
                KeyHint::new("Enter", "send"),
                KeyHint::new("Shift+Enter", "newline"),
                KeyHint::new("Ctrl+P", "command palette"),
                KeyHint::new("/", "commands"),
                KeyHint::new("Ctrl+,", "settings"),
            ],
        }),
    }
}
