//! The view model the terminal frontend draws. The live CLI keeps one `AppState` and updates it
//! from core events and input; the snapshot tool builds one from a fixture. Both draw it with
//! `view::draw`, so a fixture screen is exactly what the CLI shows in that state.

/// Everything on screen.
#[derive(Debug, Clone)]
pub struct AppState {
    pub header: HeaderInfo,
    pub mode: Mode,
    /// The logo's eyes; the live CLI closes them for the launch blink (images::eyes_at).
    pub eyes: crate::images::Eyes,
    /// Whether the terminal draws pictures. Without them the header carries no logo: at one
    /// row the text cells cannot draw the tool well (operator, 2026-11-01).
    pub pictures: bool,
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
    /// Running sessions: one window, many sessions as tabs (journal#137).
    Session(Workspace),
}

/// The sessions open in one window. The active tab's session fills the screen; the others
/// keep running behind their tabs.
#[derive(Debug, Clone, Default)]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// A notification over the top right of the transcript (an inbound message that arrived
    /// while the agent was busy).
    pub toast: Option<Toast>,
}

impl Workspace {
    pub fn one(view: SessionView, title: &str) -> Workspace {
        Workspace { tabs: vec![Tab { title: title.into(), icon: TabIcon::Prompt, mark: TabMark::None, view }], active: 0, toast: None }
    }

    pub fn view(&self) -> &SessionView {
        &self.tabs[self.active].view
    }

    pub fn view_mut(&mut self) -> &mut SessionView {
        &mut self.tabs[self.active].view
    }
}

#[derive(Debug, Clone)]
pub struct Tab {
    pub title: String,
    pub icon: TabIcon,
    pub mark: TabMark,
    pub view: SessionView,
}

/// The glyph before a tab's title (session-tabs-*: `>_` a coding session, `↔` a peer agent,
/// a document session, a locked one waiting for permission).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabIcon {
    Prompt,
    Agent,
    Doc,
    Lock,
}

/// What a background tab wants the user to notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabMark {
    None,
    /// The session is generating.
    Busy,
    /// Messages arrived since the tab was last shown.
    Unread(u32),
    /// The session waits for the user (a permission prompt).
    Attention,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: InboundKind,
    pub from: String,
    pub text: String,
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
    /// Items hidden below the bottom of the transcript (0: the newest item is in view).
    pub scroll: usize,
    /// Minutes since midnight for the next message's time stamp (debug feed and fixtures).
    pub clock: u32,
    /// The agent is producing a reply (`generating…` on the last assistant turn).
    pub generating: bool,
}

/// One entry of the transcript.
#[derive(Debug, Clone)]
pub enum Item {
    /// The card a fresh session opens with.
    Welcome { title: String, tagline: String },
    /// A card of tips: a label and bullet lines.
    Tips { label: String, lines: Vec<String> },
    /// A message of the user or the assistant; an assistant turn carries what it did (tool
    /// calls, journal and mind-map events) drawn inside its card.
    Turn { role: Role, time: String, lines: Vec<String>, attached: Vec<Item> },
    /// One tool call.
    Tool(ToolCall),
    /// A message that reached the session from outside (inbound-messages-*).
    Inbound(Inbound),
    /// A journal entry the agent wrote (journal#136).
    Journal(JournalEvent),
    /// A mind map was created; it has no nodes yet, so only the event shows.
    MapCreated(MapCreated),
    /// A mind map changed: the event line and the map itself.
    Map(MapEvent),
}

#[derive(Debug, Clone)]
pub struct Inbound {
    pub kind: InboundKind,
    /// Who sent it: the Telegram user, the peer session's nick, the callback label.
    pub from: String,
    pub time: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InboundKind {
    Telegram,
    /// A dictated message: its length and the transcript in `text`.
    Voice { duration: String },
    /// Agent-to-agent: the sending session, and the a2a route when both sessions are open here.
    Agent { session: String, route: Option<String> },
    /// An HTTP or socket callback: transport and the reply status.
    Callback { transport: String, status: String },
    /// The session's own journal reminder.
    Reminder { id: u32, every: String },
}

#[derive(Debug, Clone)]
pub struct JournalEvent {
    pub id: u32,
    pub category: String,
    pub tags: Vec<String>,
    pub date: String,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct MapCreated {
    pub id: u32,
    pub title: String,
    /// `flowchart` or `mindmap`, and `shared` or `private`.
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct MapEvent {
    pub id: u32,
    /// What changed, e.g. `+node n4, +edge n2->n4 (ref #137)`.
    pub change: String,
    pub date: String,
    pub map: FlowMap,
}

/// A flowchart as the transcript draws it: nodes in reading order and the arrows between them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowMap {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowNode {
    pub id: String,
    pub text: String,
    pub shape: NodeShape,
    /// Changed by the event being shown.
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeShape {
    Box,
    Decision,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub label: Option<String>,
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
    pub icon: ToolIcon,
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
    /// Command output: numbered file lines (`cat`), a stderr tail, or live output. Drawn as a
    /// card (tool-calls-*), folded after `lines` with `+N more lines`.
    Output { label: Option<String>, lines: Vec<String>, numbered: bool, more: usize },
    /// An outgoing message shown as what was sent (telegram_send).
    Bubble(String),
    /// A one-line result under the card header (agent_send, journal_add).
    Note(String),
}

/// The glyph a tool card starts with (tool-calls-*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolIcon {
    #[default]
    None,
    Shell,
    Send,
    Agent,
    Journal,
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
