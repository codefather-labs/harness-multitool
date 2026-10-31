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
    /// Shown first on launch (journal: tui-splash).
    Splash(SplashView),
    /// After the splash: recent sessions and quick actions (journal: tui-flow).
    Startup(StartupView),
    Session(SessionView),
}

/// The product line shown big on the splash and startup screens.
#[derive(Debug, Clone, Default)]
pub struct Brand {
    pub name: String,
    pub version: String,
    pub slogan: String,
}

#[derive(Debug, Clone, Default)]
pub struct SplashView {
    pub brand: Brand,
    pub heading: String,
    pub lines: Vec<String>,
    pub button: String,
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupPanel {
    Sessions,
    #[default]
    Actions,
}

#[derive(Debug, Clone, Default)]
pub struct StartupView {
    pub brand: Brand,
    pub sessions: Vec<RecentSession>,
    pub actions: Vec<QuickAction>,
    pub focus: StartupPanel,
    pub selected_session: usize,
    pub selected_action: usize,
    /// Label/value pairs on the status line, e.g. ("THEME", "dark").
    pub status: StatusLine,
    /// The state on the right of the status line, e.g. "ready".
    pub ready: String,
    pub tips: Vec<Vec<TipPart>>,
    pub tip: usize,
}

#[derive(Debug, Clone)]
pub struct RecentSession {
    pub title: String,
    pub path: String,
    pub branch: String,
    pub age: String,
    pub model: String,
}

/// What a quick action does when chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    NewSession,
    Resume,
    Settings,
    Tour,
    Help,
}

#[derive(Debug, Clone)]
pub struct QuickAction {
    pub key: char,
    pub icon: crate::images::Icon,
    pub title: String,
    pub description: String,
    pub kind: ActionKind,
}

/// A piece of a tip: plain text or an inline command chip.
#[derive(Debug, Clone)]
pub struct TipPart {
    pub text: String,
    pub code: bool,
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
