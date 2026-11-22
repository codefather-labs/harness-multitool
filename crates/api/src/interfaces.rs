//! The pluggable interfaces (docs/spec/interfaces.md). Each trait states only what the loop
//! needs of a component; a component's boundary is in its doc comment.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{HookOutcome, HookPointKind, LogEntry};
use crate::event::Event;
use crate::model::{Capability, Message, ModelEvent, ModelInfo, ModelRequest, ToolCall, ToolOutput, ToolSpec, TurnEnd, Usage};
use crate::platform::Platform;
use crate::state::SessionState;
use crate::{BoxFuture, PluginResult};

/// A credential. It prints as `[redacted]` and cannot be serialised, so it cannot reach a log,
/// an event or an error message by accident (INV-6).
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Secret(value.into())
    }

    /// The value, for the outgoing request and nothing else.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// IF-2. The only component that sees secrets.
pub trait Auth {
    fn credentials(&self, provider: &str) -> BoxFuture<'_, PluginResult<Secret>>;
}

/// One message for an agent backend.
pub struct BackendRequest<'a> {
    pub prompt: &'a str,
    /// Its own session, to go on with.
    pub session: Option<&'a str>,
    /// What the harness adds to the backend's own instructions.
    pub system: Option<&'a str>,
    pub plan_mode: bool,
    /// `session` is someone else's: go on in a copy of it, under a new id (ADR 0022).
    pub fork: bool,
    /// What it said its session has cost so far.
    pub cost_total_micro_usd: u64,
    pub turn: u32,
    /// The user has asked it to stop since this was last asked.
    pub stopped: &'a dyn Fn() -> bool,
    /// What the user has sent since, one at a time: it goes to the backend during the run.
    pub next_message: &'a dyn Fn() -> Option<String>,
    /// It began a turn by itself, between runs: there is no prompt to send, only that turn to read.
    pub woken: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendEnd {
    Done,
    Stopped,
    /// Stopped with these messages, sent during the run, never taken: they are in the log, and the
    /// next run takes them, and not what was stopped.
    StoppedBeforeMessages(Vec<String>),
}

/// IF-17. Runs a session's turns on its own instead of the engine with a `Provider`, and reports
/// what it does as the log's events. `sink` answers false when the user has stopped it.
pub trait AgentBackend {
    fn name(&self) -> &str;
    /// What it knows of the conversation is out of date: the next run starts from its form.
    fn reset(&self) {}
    /// Its own process, if it keeps one between runs: that one does not hold a conversation
    /// against this session.
    fn own_process(&self) -> Option<u32> {
        None
    }
    /// Reads what it has said since its last run, giving what is only news to `sink`. True when it
    /// has begun a turn by itself, which a run with `woken` then reads.
    fn between_runs(&self, _sink: &mut dyn FnMut(Event)) -> bool {
        false
    }
    fn run<'a>(&'a self, request: BackendRequest<'a>, sink: &'a mut dyn FnMut(Event) -> bool) -> BoxFuture<'a, PluginResult<BackendEnd>>;
}

/// IF-1. Network to its own endpoint only; credentials only through `Auth`.
pub trait Provider {
    fn name(&self) -> &str;
    fn models(&self) -> Vec<ModelInfo>;
    /// Streams a turn into `sink`. When `sink` returns `false` the user has stopped: end the
    /// turn at once with `StopReason::Stopped`.
    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub provider: String,
    pub model: String,
    pub max_tokens: u32,
}

/// What a router sees of the task so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spent {
    pub turns: u32,
    pub usage: Usage,
    pub cost_micro_usd: u64,
    pub elapsed_ms: u64,
}

/// IF-3. Reads usage; cannot call tools. An `Err` means the budget ran out.
pub trait Router {
    fn route(&self, spent: &Spent) -> Result<Route, String>;
}

/// What a tool gets to work with: the platform behind the capabilities `Policy` approved.
pub struct ToolContext {
    pub platform: Platform,
    pub granted: Vec<Capability>,
    /// The project's root; relative paths resolve against it.
    pub root: String,
}

/// IF-4. Only the capabilities it declared and `Policy` approved (INV-3).
pub trait Tool {
    fn spec(&self) -> ToolSpec;
    /// What this call needs. A call that needs nothing declared here must not touch anything.
    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String>;
    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
    Ask,
}

/// A user's answer to a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    AllowOnce,
    AlwaysAllow,
    Deny,
}

/// IF-5. Sees calls and declarations, not secrets.
pub trait Policy {
    fn decide(&self, call: &ToolCall, capabilities: &[Capability]) -> Decision;
    /// Remembers an answer within its scope.
    fn remember(&self, call: &ToolCall, capabilities: &[Capability], answer: Answer);
}

/// The moment a hook runs on (IF-6).
#[derive(Debug, Clone, PartialEq)]
pub enum HookPoint<'a> {
    SessionStart,
    /// The first message after coming back to a stored conversation.
    SessionResume,
    UserPromptSubmit { text: &'a str },
    PreToolUse { call: &'a ToolCall },
    PostToolUse { call: &'a ToolCall, output: &'a ToolOutput },
    Stop,
    PostCompact,
    PlanEnter,
    PlanExit,
    StepDebugEnter,
    StepDebugExit,
}

impl HookPoint<'_> {
    pub fn kind(&self) -> HookPointKind {
        match self {
            HookPoint::SessionStart => HookPointKind::SessionStart,
            HookPoint::SessionResume => HookPointKind::SessionResume,
            HookPoint::UserPromptSubmit { .. } => HookPointKind::UserPromptSubmit,
            HookPoint::PreToolUse { .. } => HookPointKind::PreToolUse,
            HookPoint::PostToolUse { .. } => HookPointKind::PostToolUse,
            HookPoint::Stop => HookPointKind::Stop,
            HookPoint::PostCompact => HookPointKind::PostCompact,
            HookPoint::PlanEnter => HookPointKind::PlanEnter,
            HookPoint::PlanExit => HookPointKind::PlanExit,
            HookPoint::StepDebugEnter => HookPointKind::StepDebugEnter,
            HookPoint::StepDebugExit => HookPointKind::StepDebugExit,
        }
    }
}

/// IF-6. Through events only (INV-7).
pub trait Hook {
    fn name(&self) -> &str;
    /// Whether it runs on this point at all; one that does not leaves no trace in the log.
    fn applies(&self, _point: &HookPoint<'_>) -> bool {
        true
    }
    /// A built-in that often has nothing to say leaves no trace in the log when it says nothing.
    fn quiet_when_ok(&self) -> bool {
        false
    }
    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome>;
}

/// A new journal entry (CON-21).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JournalEntry {
    pub body: String,
    pub category: String,
    pub tags: Vec<String>,
    pub flags: Vec<String>,
    pub salience: Option<String>,
    pub feature: Option<String>,
    /// An entry that has to be closed before this one can be (CON-91).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_by: Option<u64>,
    /// Where the evidence this entry carries is: a file, log, screenshot or URL.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<String>,
}

/// Which entries `Memory::journal_list` returns.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JournalFilter {
    pub category: Option<String>,
    pub flags: Vec<String>,
    pub open: Option<bool>,
    pub limit: usize,
}

/// What an agent does with the journal and its maps beyond writing and reading an entry. Its answer
/// is the store's own JSON, for the model to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoryOp {
    JournalList { category: Option<String>, tags: Vec<String>, flags: Vec<String>, open: Option<bool>, feature: Option<String>, offset: usize, limit: usize },
    JournalAgenda { limit: usize },
    JournalMark { id: u64, flags: Vec<String> },
    /// `id` cannot be closed until `by` is (CON-91).
    JournalBlock { id: u64, by: u64 },
    JournalUnblock { id: u64, by: u64 },
    /// What closing this entry set free.
    JournalReleased { id: u64 },
    JournalUnmark { id: u64, flags: Vec<String> },
    JournalReopen { id: u64, flags: Vec<String> },
    /// Closes a todo whose work was dropped: no evidence is needed, the resolution says why.
    JournalAbandon { id: u64, resolution: String },
    JournalBranch { id: u64 },
    /// The entries linked to `ids`, as id, title and link, a page of them (CON-118). `limit` 0 is one whole page.
    JournalGraph { ids: Vec<u64>, depth: u64, offset: usize, limit: usize },
    JournalLink { from: u64, to: u64, kind: String },
    JournalTags { category: Option<String> },
    MapEdit { map: u64, node: u64, text: Option<String>, kind: Option<String> },
    MapNodeState { map: u64, node: u64, state: String },
    /// `to_map`: the map of `to` when it is another map's node (CON-196).
    MapLink {
        map: u64,
        from: u64,
        to: u64,
        label: String,
        refs: Vec<u64>,
        #[serde(default)]
        to_map: Option<u64>,
    },
    MapRef { map: u64, node: u64, entries: Vec<u64> },
    MapClose { map: u64 },
}

/// A tool call's result when the user stopped it: a card knows it by this.
/// What the harness itself has left running, for a session with no agent backend to report it.
pub trait BackgroundWatch {
    /// What changed since it was last asked.
    fn check(&self) -> BackgroundNews;
    fn running(&self) -> Vec<crate::event::BackgroundTask>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackgroundNews {
    /// All that runs now, if that changed.
    pub running: Option<Vec<crate::event::BackgroundTask>>,
    /// What the agent is to be told of tasks that ended.
    pub ended: Vec<String>,
}

pub const TOOL_STOPPED: &str = "Stopped by the user.";

/// A stored journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JournalRecord {
    pub id: u64,
    pub created: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body: String,
    pub flags: Vec<String>,
    /// What has to close before this one can, of what it was blocked by: those still open.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<u64>,
    /// Of what it was blocked by, what was dropped instead of solved: to be re-read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocker_dropped: Vec<u64>,
    /// It holds work not yet closed: a todo, question, problem and the like.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub open: bool,
    /// Where its evidence is kept, answered when one entry is read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<String>,
}

/// The two map tools (ADR 0011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MapKind {
    MindMap,
    Flowchart,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MapNode {
    pub id: u64,
    pub parent: Option<u64>,
    pub kind: String,
    pub text: String,
    pub state: String,
    pub refs: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MapEdge {
    pub from: u64,
    pub to: u64,
    pub label: String,
    pub refs: Vec<u64>,
}

/// A link between a node of one map and a node of another, joining their graphs (CON-196).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MapCrossLink {
    pub from_map: u64,
    pub from_node: u64,
    pub to_map: u64,
    pub to_node: u64,
    pub label: String,
}

/// A map as a whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MapData {
    pub id: u64,
    pub title: String,
    pub kind: MapKind,
    pub direction: String,
    pub root: u64,
    pub nodes: Vec<MapNode>,
    pub edges: Vec<MapEdge>,
    /// Its links to and from other maps' nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<MapCrossLink>,
}

/// A new node: under `parent` in a mind map, after `after` in a flowchart.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NewMapNode {
    pub text: String,
    pub kind: String,
    pub parent: Option<u64>,
    pub after: Option<u64>,
    pub label: String,
    pub refs: Vec<u64>,
}

/// IF-7. Grows with its milestones.
pub trait Memory {
    fn journal_add(&self, entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>>;
    fn journal_search(&self, query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>>;
    fn journal_get(&self, id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>>;
    fn journal_close(&self, id: u64, resolution: &str) -> BoxFuture<'_, PluginResult<()>>;
    /// The specifications and constraints that bind the project, newest first.
    fn journal_binding(&self, limit: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>>;
    /// Entries by what they are: `flags` is any of, `open` narrows to open or closed items.
    fn journal_list(&self, filter: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>>;
    /// The titles of the project's open maps: (id, title).
    fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>>;
    fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>>;
    fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>>;
    fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>>;
    /// The rest of what the store can do. A memory that keeps less says so.
    fn more(&self, _op: MemoryOp) -> BoxFuture<'_, PluginResult<serde_json::Value>> {
        Box::pin(std::future::ready(Err(crate::PluginError::new("this memory does not do that"))))
    }
}

/// The context of one turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnContext {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    /// The strategy cut the conversation here for this turn on: the engine logs it.
    pub compacted: Option<Compaction>,
}

/// A cut the strategy made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    pub upto: usize,
    pub tokens_before: u64,
    pub tokens_after: u64,
}

/// IF-8. Reads memory and the session; writes nothing but the context.
pub trait ContextStrategy {
    /// `fixed_tokens`: what the request carries besides the system prompt and the messages, the
    /// tool schemas, estimated, so a budget counts the whole prompt (journal#1536).
    fn build<'a>(&'a self, state: &'a SessionState, memory: &'a dyn Memory, fixed_tokens: u64) -> BoxFuture<'a, PluginResult<TurnContext>>;
}

/// IF-9. Persists the append-only log (INV-5).
pub trait SessionStore {
    fn append<'a>(&'a self, session: &'a str, entries: &'a [LogEntry]) -> BoxFuture<'a, PluginResult<()>>;
    fn load<'a>(&'a self, session: &'a str) -> BoxFuture<'a, PluginResult<Vec<LogEntry>>>;
    fn list(&self) -> BoxFuture<'_, PluginResult<Vec<String>>>;
}

/// A task's verification protocol: commands and the outcome each must show.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Protocol {
    pub steps: Vec<String>,
    /// The specifications some step checks, for reporting the ones nothing checks (INV-11).
    pub specs: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    pub pass: bool,
    pub evidence: String,
}

/// IF-10. Runs or checks the protocol and returns the evidence. A model's claim alone never
/// passes (INV-4).
pub trait Verifier {
    fn verify<'a>(&'a self, protocol: &'a Protocol, state: &'a SessionState) -> BoxFuture<'a, PluginResult<Verification>>;
}

/// IF-11, its observing half: sees every log entry as it happens.
pub trait Frontend {
    fn on_entry(&self, entry: &LogEntry);
    /// A permission prompt; the future ends when the user answers.
    fn ask<'a>(&'a self, call: &'a ToolCall, capabilities: &'a [Capability]) -> BoxFuture<'a, Answer>;
}

/// IF-12. Addresses sessions by name and delivers messages between them.
pub trait Transport {
    fn send<'a>(&'a self, to: &'a str, message: &'a str) -> BoxFuture<'a, PluginResult<()>>;
}

/// An incoming callback: the contract of IF-13. Everything else in `fields` is free.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Callback {
    pub uuid: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(flatten)]
    pub fields: Value,
}

/// What the entry point tells a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed {
    Handled { handler: String },
    Duplicate,
    /// No handler for this type: logged, reaches no agent, the caller gets 400.
    NoHandler,
}

/// IF-13. Routes a callback by its `type`; the payload is data (INV-12).
pub trait Trigger {
    fn route<'a>(&'a self, callback: &'a Callback) -> BoxFuture<'a, PluginResult<Routed>>;
}

/// IF-14. A named package of instructions, tools and policy; can narrow what `Policy` allows.
pub trait Skill {
    fn name(&self) -> &str;
    fn instructions(&self) -> &str;
    fn tools(&self) -> Vec<String>;
}

/// IF-15. Any logic over the accumulated signals, always a bool. `true` wakes the agent.
pub trait Condition {
    fn name(&self) -> &str;
    fn check(&self, signals: &[Callback]) -> bool;
    /// The response-policy files this condition links (CON-26).
    fn policies(&self) -> Vec<String>;
}

/// What a handler wants done with a callback it handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerEffect {
    /// Nothing reaches the agent.
    Quiet,
    /// One message into the agent's input, marked as external data (INV-12).
    Agent { label: String, text: String },
}

/// IF-16. Handles callbacks of the types it serves, or a standard type when chosen for it. The
/// daemon calls it after its own checks, so it cannot turn off INV-14.
pub trait CallbackHandler {
    fn name(&self) -> &str;
    /// The custom types it serves; empty for one that only overrides standard types.
    fn types(&self) -> Vec<String>;
    fn handle<'a>(&'a self, callback: &'a Callback) -> BoxFuture<'a, PluginResult<HandlerEffect>>;
}

/// A conversation another agent keeps, as a source lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationInfo {
    pub id: String,
    pub title: String,
    pub last_ms: u64,
    pub bytes: u64,
}

/// A conversation as this harness's log, ending in `Event::ConversationImported`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedConversation {
    pub info: ConversationInfo,
    pub entries: Vec<crate::event::LogEntry>,
    /// Lines of the source that could not be read.
    pub damaged: usize,
}

/// What a form gained past a position.
#[derive(Debug, Clone, PartialEq)]
pub struct PulledForm {
    /// In the log's terms; their numbers mean nothing.
    pub entries: Vec<crate::event::LogEntry>,
    /// Where it stands now; the same one when nothing changed.
    pub cursor: String,
    /// The form was rewound behind the position: `entries` are its new branch.
    pub rewound: bool,
}

/// IF-18: a provider's own form of a conversation, read into the harness's log and written from
/// it. The log is the conversation; this is how it reaches a provider that keeps its own.
pub trait ConversationFormat {
    fn name(&self) -> &str;
    /// The conversations of this project, newest first.
    fn list(&self, root: &str) -> PluginResult<Vec<ConversationInfo>>;
    fn read(&self, root: &str, id: &str) -> PluginResult<ImportedConversation>;
    /// Writes this history as a new conversation of the provider and returns its id. The history
    /// ends where the next message will go on.
    fn write(&self, root: &str, title: &str, history: &[Message]) -> PluginResult<String>;
    /// Where the form stands now, for a position to go on from.
    fn tip(&self, root: &str, id: &str) -> PluginResult<String>;
    /// Where the form stands now, knowing it stood at `cursor`: for a position to move on from
    /// without reading what is behind it.
    fn tip_after(&self, root: &str, id: &str, cursor: &str) -> PluginResult<String>;
    /// What the form gained past `cursor`. It may be read while another program writes it.
    fn pull(&self, root: &str, id: &str, cursor: &str) -> PluginResult<PulledForm>;
    /// Appends this history at `cursor`, the form's tip, and returns the new position. Every byte
    /// already there stays. Refused if the form moved past `cursor`.
    fn append(&self, root: &str, id: &str, cursor: &str, history: &[Message]) -> PluginResult<String>;
    /// The provider's own program that has this conversation open right now, in words for the user.
    fn held_by(&self, id: &str, except: Option<u32>) -> Option<String>;
    /// Whether the form of conversation `id` is there at all: a position does not tell, since a
    /// form not written yet stands at its start (CON-138).
    fn exists(&self, root: &str, id: &str) -> bool;
}
