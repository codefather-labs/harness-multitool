//! The view model the terminal frontend draws. The live CLI keeps one `AppState` and updates it
//! from core events and input; the snapshot tool builds one from a fixture. Both draw it with
//! `view::draw`, so a fixture screen is exactly what the CLI shows in that state.

/// Everything on screen.
#[derive(Debug, Clone)]
pub struct AppState {
    pub header: HeaderInfo,
    pub mode: Mode,
}

#[derive(Debug, Clone)]
pub struct HeaderInfo {
    pub product: String,
    pub version: String,
    /// What follows the product name: the version on a fresh session, `session/<id>` later.
    pub location: String,
    pub cwd: String,
    pub branch: Option<String>,
    pub online: bool,
}

#[derive(Debug, Clone)]
pub enum Mode {
    Session(SessionView),
}

#[derive(Debug, Clone, Default)]
pub struct SessionView {
    pub transcript: Vec<Item>,
    pub status: StatusLine,
    pub composer: Composer,
}

/// One entry of the transcript.
#[derive(Debug, Clone)]
pub enum Item {
    /// The card a fresh session opens with.
    Welcome { title: String, tagline: String },
    /// A card of tips: a label and bullet lines.
    Tips { label: String, lines: Vec<String> },
    /// A message of the user or the assistant.
    Turn { role: Role, time: String, lines: Vec<String> },
    /// One tool call.
    Tool(ToolCall),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub name: String,
    /// The argument shown after the name; plain text, drawn muted except `args_strong`.
    pub args: String,
    /// Part of the arguments shown in the text colour, before `args` (e.g. `test` of `cargo test`).
    pub args_strong: Option<String>,
    pub state: ToolState,
    /// Outcome label on the right, e.g. `completed`, `3 matches`, `applied`.
    pub outcome: String,
    pub outcome_tone: Tone,
    /// Diff counts shown after the outcome: (+added, -removed).
    pub diffstat: Option<(u32, u32)>,
    pub duration: String,
    pub expanded: bool,
    pub body: ToolBody,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Done,
    Failed,
}

/// How an outcome label is coloured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Neutral,
    Error,
}

#[derive(Debug, Clone, Default)]
pub enum ToolBody {
    #[default]
    None,
    Diff(Diff),
}

#[derive(Debug, Clone)]
pub struct Diff {
    /// `@@ -18,6 +18,10 @@`
    pub hunk: String,
    /// Context shown after the hunk header, e.g. the enclosing function.
    pub context: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone)]
pub enum DiffLine {
    Removed(String),
    Added(String),
    Context(String),
}

/// The line above the composer.
#[derive(Debug, Clone, Default)]
pub struct StatusLine {
    /// Label/value pairs, e.g. ("ADAPTER", "anthropic-api").
    pub fields: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    pub text: String,
    pub placeholder: String,
    pub focused: bool,
    pub hints: Vec<KeyHint>,
}

#[derive(Debug, Clone)]
pub struct KeyHint {
    pub key: String,
    pub label: String,
}

impl KeyHint {
    pub fn new(key: &str, label: &str) -> KeyHint {
        KeyHint { key: key.into(), label: label.into() }
    }
}
