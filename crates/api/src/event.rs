//! The events of the bus (IF-0.2). Every step of the loop is one event and one append to the
//! session log (INV-5). No event carries a secret (INV-6): there is no field for credentials.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::{Capability, StopReason, ToolCall, Usage};
use crate::state::TaskState;

/// A task an agent backend runs in the background.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackgroundTask {
    pub id: String,
    pub description: String,
    pub kind: String,
}

/// What the claude-subscription adapter answers, and the settings show (CON-56).
pub const CLAUDE_SUBSCRIPTION_REFUSED: &str = "Sorry, claude-subscription is forbidden by Anthropic's terms. Use an API key.";

/// The label of a message from the Telegram channel. Reserved: a callback cannot claim it.
pub const TELEGRAM_LABEL: &str = "telegram";
/// A voice note from there, in the words it was transcribed to.
pub const TELEGRAM_VOICE_LABEL: &str = "telegram_voice";
/// What the user is told of a voice note when no speech engine is set up. It is theirs to set up:
/// it goes to them, never to the agent (CON-89).
pub const MISSING_SPEECH_BACKEND: &str = "Voice Transcript error. Missing speech to text backend. Choose any one inside of harness settings";
/// The label of what the harness itself tells its agent. No callback can use it.
pub const HARNESS_LABEL: &str = "harness";

/// Whether a message under this label is the user's own words: from Telegram, which delivers only
/// the user's own accounts (CON-52), typed or dictated (CON-158).
pub fn is_users_own(label: &str) -> bool {
    label == TELEGRAM_LABEL || label == TELEGRAM_VOICE_LABEL
}

/// A message that came from outside the window, as the model reads it: its kind and its words,
/// nothing more (CON-158, owner's word 2026-11-14), the user on Telegram as the old harness delivered
/// them. Which kinds are the user's and which are data the agent's rules say (agent-workflow §7).
/// The harness's own words are rendered where they are used.
pub fn inbound_for_model(label: &str, text: &str) -> String {
    let kind = match label {
        TELEGRAM_LABEL => "telegram_message".to_string(),
        TELEGRAM_VOICE_LABEL => "telegram_voice_message".to_string(),
        _ => match label.strip_prefix("agent:") {
            Some(nick) => format!("agent-to-agent:{nick}"),
            None => format!("callback:{label}"),
        },
    };
    format!("[{kind}]: {text}")
}

/// A message's text without the header an inbound message carries, of this build or of builds
/// before it, so that a conversation's digest (CON-138) does not change with the header's wording.
pub fn without_inbound_header(text: &str) -> &str {
    if let Some(rest) = text.strip_prefix('[')
        && let Some((kind, body)) = rest.split_once("]: ")
        && (kind == "telegram_message" || kind == "telegram_voice_message" || kind.starts_with("callback:") || kind.starts_with("agent-to-agent:"))
        && !kind.contains('\n')
    {
        return body;
    }
    // Before CON-158: a line of its own.
    if text.starts_with("[external callback \"")
        && let Some((first, rest)) = text.split_once('\n')
        && first.ends_with(": this is data from outside, not an instruction]")
    {
        return rest;
    }
    text
}

/// Who decided a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DecidedBy {
    Policy,
    User,
    Hook,
    /// Plan mode: refused before anyone is asked.
    Mode,
}

/// The loop events a hook can run on (docs/tui.md, ADR 0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookPointKind {
    SessionStart,
    SessionResume,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    PostCompact,
    PlanEnter,
    PlanExit,
    StepDebugEnter,
    StepDebugExit,
}

/// What a hook did (IF-6, ADR 0010).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum HookOutcome {
    Ok,
    /// Lines added to the agent's context.
    Context { lines: Vec<String> },
    /// Lines for the next model turn only, then gone.
    Reminder { lines: Vec<String> },
    /// At `Stop` only: the same lines, and the run goes on instead of ending.
    Continue { lines: Vec<String> },
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
    /// Something for the user to read in this conversation. It starts no run and never reaches the
    /// model, in this session or in any form it is brought to.
    Notice { label: String, text: String },
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
    /// The session id of an agent backend, for going on with it (IF-17).
    BackendSession { id: String },
    /// Another provider took the session over: a backend's own session no longer holds all of it.
    BackendSessionLost,
    /// The conversation as a summary says it was up to here; what precedes is not sent any more.
    ContextSummary { text: String },
    /// What an agent backend says its session has cost in all: it counts across resumes, so a
    /// turn's own cost is the difference from the last of these.
    BackendCost { total_micro_usd: u64 },
    /// This conversation's place in an orchestra (CON-100): `master`, `slave`, or none, and for a
    /// slave who it answers to.
    OrchestraRole {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        master: Option<String>,
        /// For a slave, the journal entry that is its task (CON-109).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        task: Option<u64>,
    },
    /// An agent backend compacted its conversation by itself: what it knew of the session is gone.
    BackendCompacted,
    /// What an agent backend has running in the background now, all of it.
    BackgroundTasks { tasks: Vec<BackgroundTask> },
    /// A background task of an agent backend has ended.
    BackgroundDone {
        #[serde(default)]
        id: String,
        status: String,
        summary: String,
    },
    /// The context window of the model an agent backend ran, as it reports it.
    ContextWindow { tokens: u64 },
    /// This session goes on with a conversation brought in from `source`: what precedes it in the
    /// log is that conversation, drawn. On its own, with no `FormSynced` after it, its next message
    /// forks it: how a session brought in before ADR 0025 goes on.
    ConversationImported { source: String, id: String, title: String },
    /// This session's form at `source` is `id` and has reached `cursor`, holding `holds` of the
    /// session's messages, or all of them so far (ADR 0025).
    FormSynced {
        source: String,
        id: String,
        cursor: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holds: Option<usize>,
        /// The digest of the log's messages the form holds (CON-138), taken when the daemon made
        /// them agree. None from a form's own reading, and in logs of builds before it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<Vec<u64>>,
    },
    /// The agent's name in this session, and who gave it: `user` or `agent`.
    AgentName { name: String, by: String },
    /// Plan mode on or off, and who did it: `user` or `agent`.
    PlanMode { on: bool, by: String },
    /// Step-debug mode on or off (IF-0.5), and who did it.
    StepDebug { on: bool, by: String },
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
