//! The pluggable interfaces (docs/spec/interfaces.md). Each trait states only what the loop
//! needs of a component; a component's boundary is in its doc comment.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{HookOutcome, HookPointKind, LogEntry};
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
    UserPromptSubmit { text: &'a str },
    PreToolUse { call: &'a ToolCall },
    PostToolUse { call: &'a ToolCall, output: &'a ToolOutput },
    Stop,
    PostCompact,
}

impl HookPoint<'_> {
    pub fn kind(&self) -> HookPointKind {
        match self {
            HookPoint::SessionStart => HookPointKind::SessionStart,
            HookPoint::UserPromptSubmit { .. } => HookPointKind::UserPromptSubmit,
            HookPoint::PreToolUse { .. } => HookPointKind::PreToolUse,
            HookPoint::PostToolUse { .. } => HookPointKind::PostToolUse,
            HookPoint::Stop => HookPointKind::Stop,
            HookPoint::PostCompact => HookPointKind::PostCompact,
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
}

/// A stored journal entry.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JournalRecord {
    pub id: u64,
    pub created: String,
    pub category: String,
    pub tags: Vec<String>,
    pub body: String,
    pub flags: Vec<String>,
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
}

/// A new node: under `parent` in a mind map, after `after` in a flowchart.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
    fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>>;
    fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>>;
    fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>>;
}

/// The context of one turn.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TurnContext {
    pub system: Option<String>,
    pub messages: Vec<Message>,
}

/// IF-8. Reads memory and the session; writes nothing but the context.
pub trait ContextStrategy {
    fn build<'a>(&'a self, state: &'a SessionState, memory: &'a dyn Memory) -> BoxFuture<'a, PluginResult<TurnContext>>;
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

/// IF-16. Handles callbacks of the types it is bound to; cannot turn off INV-14.
pub trait CallbackHandler {
    fn name(&self) -> &str;
    fn types(&self) -> Vec<String>;
    fn handle<'a>(&'a self, callback: &'a Callback) -> BoxFuture<'a, PluginResult<()>>;
}
