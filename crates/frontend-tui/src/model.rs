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
#[allow(clippy::large_enum_variant)] // one value per app, moved only between screens
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
    /// What a new tab runs on until told otherwise: the default adapter and model (CON-104).
    pub new_tab_runs_on: Option<(String, String)>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// A notification over the top right of the transcript (an inbound message that arrived
    /// while the agent was busy).
    pub toast: Option<Toast>,
    /// A modal window over the session: it takes the keys until it closes.
    pub overlay: Option<Overlay>,
    /// A map being dragged with the mouse.
    pub drag: Option<Drag>,
    /// Journal entries the transcript can unfold (the project's journal, as far as the session
    /// has fetched it).
    pub journal: Vec<JournalEvent>,
    /// The tabs are real sessions: a plain message goes to the agent, not into the transcript.
    pub live: bool,
    /// What the user did that the running sessions must hear about; the app drains it.
    pub outbox: Vec<Outgoing>,
    /// What the settings' Callbacks tab shows when opened, as the app knows it.
    pub callbacks: Option<CallbacksPage>,
    pub telegram: Option<TelegramPage>,
    pub voice: Option<VoicePage>,
    pub memory: Option<MemoryPage>,
    /// The profiles, as the running harness reads them (CON-125).
    pub profiles: Option<ProfilesPage>,
    /// The sign-in state as the running harness knows it.
    pub auth: Option<AuthPage>,
}

/// A user's action for a running session, by the tab's id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    Prompt { tab: u64, text: String },
    /// A message that is a task: done only when its checks pass (INV-4).
    Task { tab: u64, text: String, steps: Vec<String> },
    /// Fetch this speech model.
    DownloadVoiceModel { model: String },
    /// The Speech to text page saved.
    SaveVoice { engine: Option<String>, model: String, device: Option<VoiceDevice> },
    /// The input devices' dropdown opened: what is there now.
    ListVoiceDevices,
    /// The page's test: the microphone on.
    StartVoiceTest { model: String, device: Option<String> },
    /// The microphone off: what it heard goes to the engine.
    StopVoiceTest,
    /// The Telegram page saved: a new token if one was typed, and who may speak.
    SaveTelegram { token: Option<String>, users: Vec<i64> },
    /// The Memory page saved: the most nodes a map may hold, 0 for none (CON-114).
    SaveMapLimit { limit: u64 },
    /// The Memory page saved: the most linked entries a journal answer holds, 0 for no paging (CON-118).
    SaveSearchPage { limit: u64 },
    /// The Profiles tab (CON-125): a new clean profile, one deleted, this window moved onto one, and
    /// Claude Code signed in or out of one, which hands the terminal to `claude auth`.
    ProfileAdd { name: String },
    ProfileDelete { name: String },
    ProfileUse { name: String },
    ProfileClaude { name: String, login: bool },
    /// The Login button of gpt-subscription, and what follows it.
    Login,
    LoginCancel,
    Logout,
    /// A key typed in the Auth tab, for this provider row.
    SaveKey { provider: String, key: harness_api::Secret },
    /// `/init`: create the docs tree and `.harness/`.
    Init { tab: u64 },
    /// `/stepdebug`: step-debug mode on or off.
    StepDebug { tab: u64, on: bool },
    /// Up on an empty composer: a queued message taken back.
    Unqueue { tab: u64, text: String },
    /// `/nick`: name the agent.
    Nick { tab: u64, name: String },
    /// `/plan`: plan mode on or off.
    Plan { tab: u64, on: bool },
    /// `/task trust`: approve the project's checks as last shown.
    ApproveChecks { tab: u64 },
    /// The answer to a permission prompt: 0 allow once, 1 always allow, 2 deny.
    Answer { tab: u64, choice: usize },
    Stop { tab: u64 },
    /// The Callbacks tab saved: each standard type's implementation, and what is switched off.
    SaveCallbacks { handlers: Vec<(String, String)>, disabled: Vec<String> },
    /// Rebuild and swap the binary (ADR 0015).
    Reload,
    /// Make the daemon read the project's callback files again.
    ReloadCallbacks,
    /// Settings saved: the provider's option name and the model, for new sessions.
    SaveSettings { provider: String, model: String, context: Option<u64>, delegate: (bool, bool) },
}

/// A press on a map, until the release: where it started, what it started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Drag {
    pub item: usize,
    pub sub: Option<usize>,
    pub at: (u16, u16),
    pub pan: (u16, u16),
    pub limit: (u16, u16),
    /// The part pressed (a ref, a node, an arrow): a release without moving unfolds it.
    pub part: Option<MapElement>,
    pub moved: bool,
}

#[derive(Debug, Clone)]
// One overlay at a time, held by value: its size costs nothing that matters here.
#[allow(clippy::large_enum_variant)]
pub enum Overlay {
    Permission(PermissionView),
    Settings(SettingsView),
    /// The daemon is bringing a conversation in line: the window waits and takes no input (CON-81).
    Syncing(SyncingView),
}

#[derive(Debug, Clone)]
pub struct SyncingView {
    /// What is being done, in a few words.
    pub what: String,
    pub started_ms: u64,
    pub now_ms: u64,
}

/// A tool call waiting for the user's approval (permission-*; IF-5 Policy asked).
#[derive(Debug, Clone)]
pub struct PermissionView {
    pub tool: String,
    /// What the agent wants, in a sentence.
    pub summary: String,
    pub command: String,
    pub workdir: String,
    pub scope: String,
    pub policy: String,
    /// The button in focus: 0 allow once, 1 always allow, 2 deny.
    pub choice: usize,
    /// The tab of the running session that asks; `None` for the debug sample.
    pub tab: Option<u64>,
    /// Just opened: only Deny answers yet. A prompt can open at any moment, started by a callback
    /// or a channel, and keys meant for the composer must not approve it.
    pub locked: bool,
}

/// The settings window (settings-overlay-*): sections on the left, the section's page on the
/// right.
#[derive(Debug, Clone)]
pub struct SettingsView {
    pub sections: Vec<String>,
    pub section: usize,
    pub focus: SettingsPane,
    pub providers: Vec<ProviderOption>,
    /// The provider chosen.
    pub provider: usize,
    /// The row under the cursor in the page: providers first, then the fields.
    pub cursor: usize,
    pub model: String,
    pub timeout_s: u32,
    pub fallback: bool,
    pub callbacks: CallbacksPage,
    pub telegram: TelegramPage,
    pub voice: VoicePage,
    pub memory: MemoryPage,
    pub profiles: ProfilesPage,
    pub auth: AuthPage,
    /// The model name is being typed.
    pub editing_model: bool,
    /// The model dropdown is open, on this row.
    pub model_menu: Option<usize>,
    /// The provider row and model as last saved, to tell if closing would lose a choice.
    pub saved: (usize, String),
    /// A run is on: the adapter and model cannot be changed until it ends (CON-79).
    pub run_on: bool,
    /// The context window chosen for this provider; none is the model's own.
    pub context: Option<u64>,
    /// The context window dropdown is open, on this row.
    pub context_menu: Option<usize>,
    /// The choice as last saved.
    pub saved_context: Option<u64>,
    /// Claude Code runs in the user's own environment: their hooks, skills, servers and browser.
    pub delegate_own: bool,
    /// Claude Code asks no permissions. Only ever the user's own choice (CON-70).
    pub delegate_skip: bool,
    /// Those two as last saved.
    pub saved_delegate: (bool, bool),
}

/// The provider row of a ChatGPT plan.
pub const PLAN_PROVIDER: &str = "gpt-subscription";

/// The provider row of the Claude Code delegate, in a build that has it.
pub const DELEGATE_PROVIDER: &str = "claude-code";

/// A model on offer: its id, and what it costs and holds, as a line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelChoice {
    pub id: String,
    pub detail: String,
    /// Its context window, where the server says.
    pub context: Option<u64>,
}

/// The chosen provider's sign-in, on the Provider page of the settings.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AuthPage {
    /// What the chosen provider's key comes from: (provider row, where).
    pub keys: Vec<(String, String)>,
    /// `Signed in as ...` or `Not signed in`, for gpt-subscription.
    pub plan_status: String,
    pub signed_in: bool,
    /// This build has the Claude Code delegate (ADR 0020).
    pub delegate: bool,
    /// A browser sign-in is under way.
    pub pending: bool,
    /// The latest outcome, or the address to open.
    pub message: Vec<String>,
    /// The models this plan can use.
    pub models: Vec<ModelChoice>,
    /// What other providers offer: (provider row, models).
    pub provider_models: Vec<(String, Vec<ModelChoice>)>,
    /// A key being typed.
    pub entry: Option<String>,
    /// The context windows chosen so far: (provider id, tokens).
    pub contexts: Vec<(String, u64)>,
    /// How Claude Code's row is set: (own environment, no permissions).
    pub delegate_row: (bool, bool),
}

/// Prints no key.
impl std::fmt::Debug for AuthPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthPage").field("plan_status", &self.plan_status).field("pending", &self.pending).field("typing_a_key", &self.entry.is_some()).finish()
    }
}

/// The Callbacks tab of the settings (settings-callbacks-*, ADR 0015).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallbacksPage {
    pub standard: Vec<StandardRow>,
    pub custom: Vec<CustomRow>,
    pub conditions: Vec<ConditionRow>,
    /// The daemon's state, as the page's last line says it.
    pub daemon: String,
    pub cursor: usize,
    /// The standard row whose implementations are unfolded.
    pub open: Option<usize>,
}

impl CallbacksPage {
    pub fn rows(&self) -> usize {
        self.standard.len() + self.custom.len() + self.conditions.len()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StandardRow {
    pub name: String,
    pub description: String,
    pub implementations: Vec<String>,
    pub chosen: usize,
    pub concept: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomRow {
    pub name: String,
    pub implementation: String,
    pub on: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConditionRow {
    pub name: String,
    pub listens_to: String,
    pub rule: String,
    pub policy: String,
    pub on: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsPane {
    #[default]
    Sections,
    Page,
}

#[derive(Debug, Clone)]
pub struct ProviderOption {
    pub name: String,
    pub description: String,
    /// A plugin built into the user's local build rather than shipped with the project.
    pub local_build: bool,
    /// Shown but not selectable, with this reason.
    pub disabled: Option<String>,
}

impl Workspace {
    pub fn one(view: SessionView, title: &str) -> Workspace {
        Workspace { new_tab_runs_on: None, tabs: vec![Tab::new(title, TabIcon::Prompt, TabMark::None, view)], active: 0, toast: None, overlay: None, drag: None, journal: Vec::new(), live: false, outbox: Vec::new(), auth: None, callbacks: None, telegram: None, voice: None, memory: None, profiles: None }
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
    /// Stable while the tab lives, unlike its position.
    pub id: u64,
    pub title: String,
    pub icon: TabIcon,
    pub mark: TabMark,
    pub view: SessionView,
}

impl Tab {
    pub fn new(title: &str, icon: TabIcon, mark: TabMark, view: SessionView) -> Tab {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Tab { id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed), title: title.into(), icon, mark, view }
    }
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
    /// The text under the heading.
    pub lines: Vec<String>,
    /// What the harness offers, one bullet each; a tag marks what is still an idea or a
    /// concept, so the screen does not sell a design as a shipped feature (P-8).
    pub points: Vec<SplashPoint>,
    pub button: String,
    pub hint: String,
}

#[derive(Debug, Clone, Default)]
pub struct SplashPoint {
    pub text: String,
    pub tag: Option<String>,
}

impl SplashPoint {
    pub fn new(text: &str, tag: Option<&str>) -> Self {
        SplashPoint { text: text.into(), tag: tag.map(Into::into) }
    }
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

/// What the agent is doing during a run, for the line above the status line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub state: String,
    pub started_ms: u64,
    pub now_ms: u64,
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
    /// The name above the agent's messages; ASSISTANT until it has one.
    pub agent_name: Option<String>,
    /// Plan mode is on.
    pub plan_on: bool,
    /// Debug mode is on.
    pub debug_on: bool,
    /// Its place in an orchestra: `master` or `slave` (CON-100).
    pub orchestra: Option<String>,
    /// What the agent asked and the user has not answered, with the option under the cursor.
    pub question: Option<(harness_api::question::Question, usize)>,
    /// Messages sent during the run that the agent has not seen yet.
    pub queued: Vec<harness_api::PendingMessage>,
    /// What the agent has running in the background.
    pub background: Vec<BackgroundView>,
    /// The clock of the last tick while any is on.
    pub background_now_ms: u64,
    /// What the run is doing now; `None` between runs.
    pub activity: Option<Activity>,
    /// `/task` armed the next message as a task, with these commands.
    pub task: Option<Vec<String>>,
}

/// A task running in the background, as the panel above the composer lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundView {
    pub id: String,
    pub description: String,
    pub started_ms: u64,
}

/// One instance of a flowchart among several: one card, updated in place.
#[derive(Debug, Clone)]
pub struct FlowInstanceView {
    pub map: u32,
    pub instance: String,
    pub event: FlowchartEvent,
    pub expanded: bool,
    /// While an update's pulse runs: when it ends, in the log's clock.
    pub pulse_until_ms: Option<u64>,
    /// How strong the pulse is now, 0 to 8.
    pub pulse: u8,
}

/// The reasoning of one model turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThinkingView {
    pub lines: Vec<String>,
    /// Still being written: framed, with a timer and a cursor.
    pub live: bool,
    pub secs: u64,
    /// Finished and unfolded; a finished one is folded to a line by default.
    pub expanded: bool,
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
    /// One pass of a chart when several run at once: a row, opened by the user (ADR 0012).
    FlowInstance(FlowInstanceView),
    /// The model's reasoning: inside a turn, above its answer (reasoning-*).
    Thinking(ThinkingView),
    Journal(JournalEvent),
    /// A mind map or a flowchart was created (no nodes yet: only the event shows).
    MapCreated(MapCreated),
    /// A mind map changed: the event and the map.
    MindMap(MindMapEvent),
    /// A flowchart changed: the event and the chart.
    Flowchart(FlowchartEvent),
    /// Hooks that ran on a loop event (IF-6): session start, compaction, before a tool, ...
    Hook(HookEvent),
}

#[derive(Debug, Clone)]
pub struct HookEvent {
    /// The loop event the hooks ran on, e.g. `SessionStart`, `PostCompact`, `PreToolUse`.
    pub event: String,
    /// What the event was about, e.g. the tool call a `PreToolUse` hook saw.
    pub subject: Option<String>,
    pub time: String,
    pub runs: Vec<HookRun>,
}

#[derive(Debug, Clone)]
pub struct HookRun {
    pub script: String,
    pub outcome: HookOutcome,
    pub duration: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOutcome {
    /// Ran and changed nothing.
    Ok,
    /// Added text to the agent's context (its first lines are shown).
    Context(Vec<String>),
    /// Vetoed the event (a before-event hook), with its reason.
    Blocked(String),
    /// Exited non-zero: the exit code and the stderr tail.
    Failed { code: i32, stderr: Vec<String> },
    /// Killed after the time limit.
    Timeout,
    Running,
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
    /// The harness itself telling the user something; only ever a toast.
    System,
}

#[derive(Debug, Clone)]
pub struct JournalEvent {
    pub id: u32,
    pub category: String,
    pub tags: Vec<String>,
    pub date: String,
    pub text: String,
}

/// The two map tools (ADR 0011): a mind map maps what is known about a topic, a flowchart
/// shows how something flows from one point to another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapTool {
    MindMap,
    Flowchart,
}

/// A map was created; it has no nodes yet, so only the event shows.
#[derive(Debug, Clone)]
pub struct MapCreated {
    pub tool: MapTool,
    pub id: u32,
    pub title: String,
    pub shared: bool,
}

/// A mind map changed: the event line and the map itself.
#[derive(Debug, Clone)]
pub struct MindMapEvent {
    pub id: u32,
    /// What changed, e.g. `+node n6 (ref #137)`.
    pub change: String,
    pub date: String,
    pub map: MindMap,
    /// The element whose journal entries are unfolded beside the map.
    pub open: Option<MapElement>,
    /// The one entry of the ticket shown in full; the others are folded to their header.
    pub focus: Option<u32>,
    /// How far the map is dragged: (columns, rows) scrolled into it.
    pub pan: (u16, u16),
}

/// A clickable part of a map: a journal ref of a node, a node, an arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MapElement {
    /// One journal entry, by id (a `#137` chip).
    Entry(u32),
    /// A node by index: its linked entries.
    Node(usize),
    /// An arrow by index: its linked entries.
    Arrow(usize),
    /// A reasoning block's header: a click folds or unfolds it.
    Thinking,
    /// A flowchart instance's row: a click opens or closes it.
    Instance,
    /// The map's area itself (pressing there starts a drag); how far it can be panned.
    Area(u16, u16),
}

/// A flowchart changed: the event line and the chart itself.
#[derive(Debug, Clone)]
pub struct FlowchartEvent {
    pub id: u32,
    /// What changed, e.g. `+node n4, +arrow n2->n4 (ref #137)`.
    pub change: String,
    pub date: String,
    pub chart: Flowchart,
    /// The element whose journal entries are unfolded beside the chart.
    pub open: Option<MapElement>,
    /// The one entry of the ticket shown in full; the others are folded to their header.
    pub focus: Option<u32>,
    /// How far the chart is dragged: (columns, rows) scrolled into it.
    pub pan: (u16, u16),
}

/// A tree of what is known about one topic: the first node is the root; branches join a parent
/// to a child, links join any two nodes with a label (e.g. "contradicts").
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MindMap {
    pub nodes: Vec<MindNode>,
    pub branches: Vec<Branch>,
    pub links: Vec<MindLink>,
}

/// A parent -> child branch of a mind map.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Branch {
    pub from: String,
    pub to: String,
    /// Flowcharts this branch points at (how the step from parent to child goes).
    pub flows: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MindNode {
    pub id: String,
    pub text: String,
    pub kind: MindKind,
    pub status: NodeStatus,
    /// Journal entries the node points at.
    pub refs: Vec<u32>,
    /// Flowcharts the node points at (how the thing it names works).
    pub flows: Vec<u32>,
    /// Changed by the event being shown.
    pub changed: bool,
}

/// The node kinds of a mind map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MindKind {
    Topic,
    Goal,
    Decision,
    Fact,
    Hypothesis,
    Question,
    Task,
    Idea,
    DeadEnd,
    Note,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NodeStatus {
    #[default]
    Open,
    Done,
    Dropped,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MindLink {
    pub from: String,
    pub to: String,
    pub label: String,
    /// Flowcharts this link points at.
    pub flows: Vec<u32>,
}

/// A directed flow from a start to an end, by the canon: ISO 5807 symbols and logic gates
/// joined by arrows that may carry a condition.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Flowchart {
    pub direction: FlowDirection,
    pub nodes: Vec<FlowNode>,
    pub arrows: Vec<Arrow>,
}

/// Which way the flow runs on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FlowDirection {
    /// Top to bottom (the backend's default).
    Down,
    /// Left to right.
    #[default]
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FlowNode {
    pub id: String,
    pub text: String,
    pub symbol: Symbol,
    pub refs: Vec<u32>,
    /// Changed by the event being shown.
    pub changed: bool,
    /// The node a live run is at now (ADR 0012): outlined in the accent.
    pub active: bool,
}

/// ISO 5807 symbols and logic gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Symbol {
    Start,
    End,
    Process,
    Decision,
    Io,
    Subprocess,
    Document,
    Data,
    Manual,
    Prep,
    Connector,
    Delay,
    Note,
    Gate(Gate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Gate {
    And,
    Or,
    Not,
    Nand,
    Nor,
    Xor,
    Xnor,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Arrow {
    pub from: String,
    pub to: String,
    /// A condition, e.g. "yes" or "no".
    pub label: Option<String>,
    /// Journal entries the arrow points at.
    pub refs: Vec<u32>,
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
    /// Where its spinner is, while it runs: the clock of the last tick.
    pub spin_ms: u64,
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

/// A speech model on the settings' page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceModel {
    pub engine: String,
    pub name: String,
    pub size: String,
    /// Downloaded.
    pub there: bool,
}

/// An input device on the settings' page.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VoiceDevice {
    pub id: String,
    pub name: String,
}

/// How the page's test stands.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum VoiceTest {
    #[default]
    Idle,
    /// The microphone is on: how loud it is now, 0 to 1, and for how long so far.
    Listening { level: f32, seconds: usize },
    /// What was said is being turned into words.
    Hearing,
    Heard(String),
}

/// The settings' Speech to text page.
#[derive(Debug, Clone, Default)]
pub struct VoicePage {
    /// The chosen engine's id, none for off.
    pub engine: Option<String>,
    pub models: Vec<VoiceModel>,
    /// The chosen model's name.
    pub model: String,
    /// The runtime is here, or it comes with the first download: what it comes to, if it does.
    pub runtime_size: Option<String>,
    /// The chosen input device, none for the system's default.
    pub device: Option<VoiceDevice>,
    /// What the system listed when the dropdown last opened.
    pub devices: Vec<VoiceDevice>,
    /// What the system's default is named, when it says.
    pub default_device: Option<String>,
    /// 0 the engine, 1 the model, 2 the input device, 3 the download, 4 the test.
    pub cursor: usize,
    /// The dropdown of the row under the cursor is open, at this choice.
    pub menu: Option<usize>,
    /// A download is running: how far it has got.
    pub downloading: Option<String>,
    pub test: VoiceTest,
    /// How it stands, in words.
    pub status: String,
}

impl VoicePage {
    pub fn models_of_engine(&self) -> Vec<&VoiceModel> {
        self.models.iter().filter(|m| Some(&m.engine) == self.engine.as_ref()).collect()
    }

    pub fn chosen(&self) -> Option<&VoiceModel> {
        self.models_of_engine().into_iter().find(|m| m.name == self.model)
    }

    /// Choosing an engine chooses a model of it: one already here if there is one.
    pub fn choose_engine(&mut self, engine: Option<String>) {
        self.engine = engine;
        if self.chosen().is_none() {
            let of_engine = self.models_of_engine();
            let first = of_engine.iter().find(|m| m.there).or(of_engine.first());
            self.model = first.map_or(String::new(), |m| m.name.clone());
        }
    }
}

/// The settings' Memory page: the most nodes a map may hold (CON-114), the page of linked entries (CON-118).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MemoryPage {
    /// As saved in the config file; 200 when none is set.
    pub map_max_nodes: u64,
    /// As saved in the config file; 100 when none is set.
    pub search_max_linked: u64,
    /// The row under the cursor: 0 the map limit, 1 the page of linked entries.
    pub cursor: usize,
    pub editing: bool,
    /// What is typed while editing; the saved value is shown otherwise.
    pub typed: String,
    pub status: String,
}

impl std::fmt::Debug for MemoryPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryPage").field("map_max_nodes", &self.map_max_nodes).field("search_max_linked", &self.search_max_linked).field("editing", &self.editing).finish()
    }
}

/// A line of the Profiles tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileRow {
    pub name: String,
    pub provider: String,
    pub model: String,
    /// Claude Code signed in: none when it could not be asked.
    pub claude: Option<bool>,
    pub chatgpt: bool,
    /// This window runs on it.
    pub current: bool,
    /// Its folder, shown by Edit.
    pub folder: String,
}

/// What the Profiles tab is doing besides showing the list.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ProfilesMode {
    #[default]
    List,
    /// Add: the new name being typed.
    Naming(String),
    /// Delete asked; Enter or y confirms.
    Deleting,
    /// The keyboard is on the card's Claude Code button, below the list (journal#785).
    Editing,
}

/// The settings' Profiles tab (CON-125): the profiles as one list, the buttons beside it.
#[derive(Debug, Clone, Default)]
pub struct ProfilesPage {
    pub rows: Vec<ProfileRow>,
    pub cursor: usize,
    pub mode: ProfilesMode,
    pub status: String,
}

/// The settings' Telegram page. A saved token is never here, only whether there is one.
#[derive(Clone, Default)]
pub struct TelegramPage {
    pub token_saved: bool,
    /// What has been typed for a new token, drawn only as dots.
    pub token: String,
    pub users: String,
    /// 0 the token, 1 the users.
    pub cursor: usize,
    pub editing: bool,
    /// How the channel stands, in words.
    pub status: String,
}

impl std::fmt::Debug for TelegramPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TelegramPage").field("token_saved", &self.token_saved).field("token", &"<hidden>").field("users", &self.users).finish()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Composer {
    pub text: String,
    pub placeholder: String,
    pub focused: bool,
    pub hints: Vec<KeyHint>,
    /// How many characters the cursor stands before the end: what is typed goes there, and text
    /// set from outside leaves it at the end.
    pub back: usize,
    /// The width of its last drawing, for moving up and down its rows.
    pub drawn_width: std::cell::Cell<u16>,
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

impl Composer {
    fn at(&mut self) -> usize {
        let len = self.text.chars().count();
        self.back = self.back.min(len);
        len - self.back
    }

    fn byte(&self, char_index: usize) -> usize {
        self.text.char_indices().nth(char_index).map_or(self.text.len(), |(byte, _)| byte)
    }

    /// Where the cursor stands, as a character index.
    pub fn cursor(&self) -> usize {
        let len = self.text.chars().count();
        len - self.back.min(len)
    }

    pub fn insert(&mut self, text: &str) {
        let at = self.at();
        let byte = self.byte(at);
        self.text.insert_str(byte, text);
    }

    pub fn backspace(&mut self) {
        let at = self.at();
        if at > 0 {
            let byte = self.byte(at - 1);
            self.text.remove(byte);
        }
    }

    pub fn delete(&mut self) {
        let at = self.at();
        if self.back > 0 {
            let byte = self.byte(at);
            self.text.remove(byte);
            self.back -= 1;
        }
    }

    pub fn left(&mut self) {
        let at = self.at();
        if at > 0 {
            self.back += 1;
        }
    }

    pub fn right(&mut self) {
        self.at();
        self.back = self.back.saturating_sub(1);
    }

    fn move_to(&mut self, index: usize) {
        self.back = self.text.chars().count().saturating_sub(index);
    }

    /// The start of the row the cursor is on, or its end: rows as they are drawn.
    pub fn home(&mut self) {
        let at = self.at();
        let rows = crate::widgets::text_rows(&self.text, self.drawn_width.get());
        let row = crate::widgets::row_of(&rows, at);
        self.move_to(rows[row].0);
    }

    pub fn end(&mut self) {
        let at = self.at();
        let rows = crate::widgets::text_rows(&self.text, self.drawn_width.get());
        let row = crate::widgets::row_of(&rows, at);
        self.move_to(rows[row].0 + rows[row].1);
    }

    /// One row up or down, keeping the column. False if there is no row that way.
    pub fn vertical(&mut self, up: bool) -> bool {
        let at = self.at();
        let rows = crate::widgets::text_rows(&self.text, self.drawn_width.get());
        let row = crate::widgets::row_of(&rows, at);
        let target = if up { row.checked_sub(1) } else { Some(row + 1).filter(|r| *r < rows.len()) };
        let Some(target) = target else { return false };
        let column = at - rows[row].0;
        self.move_to(rows[target].0 + column.min(rows[target].1));
        true
    }
}
