//! The events of the bus (IF-0.2). Every step of the loop is one event and one append to the
//! session log (INV-5). No event carries a secret (INV-6): there is no field for credentials.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Capability, StopReason, ToolCall, Usage};
use crate::state::TaskState;

/// The label of a message from the Telegram channel. Reserved: a callback cannot claim it.
pub const TELEGRAM_LABEL: &str = "telegram";

/// Who decided a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecidedBy {
    Policy,
    User,
    Hook,
}

/// The loop events a hook can run on (docs/tui.md, ADR 0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookPointKind {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PostCompact,
}

/// What a hook did (IF-6, ADR 0010).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HookOutcome {
    Ok,
    /// Lines added to the agent's context.
    Context { lines: Vec<String> },
    /// A veto of a before-event, with its reason.
    Blocked { reason: String },
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum Event {
    UserMessage { text: String },
    /// A callback for the agent (IF-13). It reaches the model marked as external data (INV-12).
    Inbound { label: String, text: String },
    TaskState { to: TaskState, reason: Option<String> },
    TurnStarted { turn: u32, provider: String, model: String },
    TextDelta { text: String },
    /// The model's reasoning, where a provider shows it. For the people watching: it never goes
    /// back to the model as text.
    ThinkingDelta { text: String },
    /// A provider block kept for replay; frontends do not show it.
    OpaqueBlock { block: Value },
    /// The model called a tool, in its place among the turn's blocks.
    ModelToolCall { call: ToolCall },
    TurnFinished { turn: u32, stop: StopReason, usage: Usage, cost_micro_usd: Option<u64> },
    ToolRequested { call: ToolCall, capabilities: Vec<Capability> },
    ToolDecision { id: String, allowed: bool, by: DecidedBy, reason: Option<String> },
    ToolResult { id: String, output: String, is_error: bool },
    HookRan { point: HookPointKind, hook: String, outcome: HookOutcome, detail: Option<Value> },
    Verification { pass: bool, evidence: String },
    /// Messages before `upto` no longer go to the model; the log keeps them.
    Compacted { upto: usize, tokens_before: u64, tokens_after: u64 },
    /// A plugin failed; the task's state says what it meant for the task.
    Error { source: String, message: String },
}

/// One record of the append-only session log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub seq: u64,
    pub at_ms: u64,
    #[serde(flatten)]
    pub event: Event,
}

/// One log entry as the line the session store keeps.
pub fn to_line(entry: &LogEntry) -> String {
    serde_json::to_string(entry).expect("a log entry always serialises")
}

/// The entry a stored line holds.
pub fn from_line(line: &str) -> Result<LogEntry, serde_json::Error> {
    serde_json::from_str(line)
}
