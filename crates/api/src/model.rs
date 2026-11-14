//! What the loop and a provider exchange: messages, tool calls, usage.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Content {
    Text { text: String },
    ToolUse { id: String, name: String, input: Value },
    ToolResult { id: String, output: String, is_error: bool },
    /// A provider's block the harness does not interpret (thinking, fallback markers): kept in
    /// order and replayed unchanged, as the provider requires.
    Opaque { block: Value },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Content>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Self {
        Message { role: Role::User, content: vec![Content::Text { text: text.into() }] }
    }
}

/// A tool as the model sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// What a turn asks of a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: u32,
}

/// What a provider streams during a turn.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelEvent {
    Text(String),
    /// Reasoning the provider shows; for display and the log.
    Thinking(String),
    ToolCall(ToolCall),
    /// A block to replay unchanged on later requests.
    Opaque(Value),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    pub fn add(&mut self, other: Usage) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The user stopped the turn while it streamed.
    Stopped,
    /// Anything else the provider reports, e.g. a refusal.
    Other(String),
}

/// How a turn ended. Cost is in millionths of a dollar, when the provider knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEnd {
    pub stop: StopReason,
    pub usage: Usage,
    pub cost_micro_usd: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub context_window: u64,
    pub supports_tools: bool,
}

/// What a tool needs in order to run (INV-3). `Policy` approves each of these before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum Capability {
    ReadPath(String),
    WritePath(String),
    Command(String),
    Host(String),
    /// The tool writes to the harness's own memory: journal, maps, charts.
    Memory,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        ToolOutput { text: text.into(), is_error: false }
    }

    pub fn error(text: impl Into<String>) -> Self {
        ToolOutput { text: text.into(), is_error: true }
    }
}

/// The digest of a conversation (CON-138): in order, one FNV-1a hash per run of one role, over the
/// role and the texts of its messages. Messages with no text (tool calls and results only) are
/// passed over, so a run goes on across them; tool calls and results never count, because a
/// program's stream and its form record one turn's calls differently, and a form joins messages
/// of one role into one. A user message counts by its last paragraph, since what the harness sends
/// a delegate may come before the user's words, after a blank line (journal#934), and without the
/// header an inbound message carries, whatever its wording (CON-158).
pub fn conversation_digest(messages: &[Message]) -> Vec<u64> {
    let mut runs: Vec<(Role, Vec<&str>)> = Vec::new();
    for message in messages {
        let texts: Vec<&str> = message.content.iter().filter_map(|c| if let Content::Text { text } = c { Some(text.trim()) } else { None }).filter(|t| !t.is_empty()).collect();
        if texts.is_empty() {
            continue;
        }
        let texts: Vec<&str> = match message.role {
            Role::User => texts.iter().map(|t| crate::event::without_inbound_header(t.rsplit("\n\n").next().unwrap_or(t).trim()).trim()).collect(),
            Role::Assistant => texts,
        };
        match runs.last_mut() {
            Some((role, run)) if *role == message.role => run.extend(texts),
            _ => runs.push((message.role, texts)),
        }
    }
    runs.into_iter().map(|(role, texts)| {
        // FNV-1a, 64 bits: the same on every build and machine.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        };
        feed(if role == Role::User { b"user" } else { b"assistant" });
        for text in texts {
            feed(b"\0");
            feed(text.as_bytes());
        }
        hash
    }).collect()
}

/// Whether `provider_ready` would give these messages back unchanged: then they can be used as they
/// are, borrowed, with no copy of a conversation that may be gigabytes (journal#955).
pub fn is_provider_ready(messages: &[Message]) -> bool {
    let mut open: Vec<&str> = Vec::new();
    let mut last: Option<Role> = None;
    for message in messages {
        match message.role {
            Role::User => {
                // Its results answer the open calls, in their order, first; no other result after.
                if last == Some(Role::User) || message.content.len() < open.len() {
                    return false;
                }
                let (answers, rest) = message.content.split_at(open.len());
                let answered = answers.iter().zip(&open).all(|(c, id)| matches!(c, Content::ToolResult { id: i, .. } if i == id));
                if !answered || rest.iter().any(|c| matches!(c, Content::ToolResult { .. })) || message.content.is_empty() {
                    return false;
                }
                open.clear();
            }
            Role::Assistant => {
                if !open.is_empty() || last == Some(Role::Assistant) {
                    return false;
                }
                open.extend(message.content.iter().filter_map(|c| if let Content::ToolUse { id, .. } = c { Some(id.as_str()) } else { None }));
            }
        }
        last = Some(message.role);
    }
    open.is_empty()
}

/// A history any provider takes, whatever it came from: roles alternate; every call is answered in
/// the next message, results first; a result with no call before it is left out. A log can hold
/// such things after a cut, a stop, or a conversation brought in from elsewhere.
pub fn provider_ready(messages: &[Message]) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(messages.len());
    let mut open: Vec<String> = Vec::new();
    let answer = |out: &mut Vec<Message>, open: &mut Vec<String>, mut user: Vec<Content>| {
        let mut results: Vec<Content> = Vec::new();
        for id in open.drain(..) {
            let found = user.iter().position(|c| matches!(c, Content::ToolResult { id: i, .. } if *i == id));
            results.push(match found {
                Some(at) => user.remove(at),
                None => Content::ToolResult { id, output: "no result in the record".to_string(), is_error: true },
            });
        }
        user.retain(|c| !matches!(c, Content::ToolResult { .. }));
        results.append(&mut user);
        if !results.is_empty() {
            match out.last_mut() {
                Some(last) if last.role == Role::User => last.content.append(&mut results),
                _ => out.push(Message { role: Role::User, content: results }),
            }
        }
    };
    for message in messages {
        match message.role {
            Role::User => answer(&mut out, &mut open, message.content.clone()),
            Role::Assistant => {
                if !open.is_empty() {
                    answer(&mut out, &mut open, Vec::new());
                }
                open.extend(message.content.iter().filter_map(|c| if let Content::ToolUse { id, .. } = c { Some(id.clone()) } else { None }));
                match out.last_mut() {
                    Some(last) if last.role == Role::Assistant => last.content.extend(message.content.iter().cloned()),
                    _ => out.push(message.clone()),
                }
            }
        }
    }
    if !open.is_empty() {
        answer(&mut out, &mut open, Vec::new());
    }
    out
}

#[cfg(test)]
mod ready_tests {
    use super::*;

    proptest::proptest! {
        /// journal#955: the check agrees with the fix on any history: unchanged exactly when ready.
        #[test]
        fn is_ready_agrees_with_provider_ready(shape in proptest::collection::vec((proptest::bool::ANY, proptest::collection::vec(0u8..5, 0..4)), 0..12)) {
            let messages: Vec<Message> = shape.iter().map(|(user, parts)| Message {
                role: if *user { Role::User } else { Role::Assistant },
                content: parts.iter().map(|p| match p {
                    0 | 1 => Content::Text { text: format!("t{p}") },
                    2 => Content::ToolUse { id: format!("c{}", p), name: "n".into(), input: serde_json::Value::Null },
                    3 => Content::ToolUse { id: "c9".into(), name: "n".into(), input: serde_json::Value::Null },
                    _ => Content::ToolResult { id: if *user { "c2".into() } else { "c9".into() }, output: "r".into(), is_error: false },
                }).collect(),
            }).collect();
            proptest::prop_assert_eq!(is_provider_ready(&messages), provider_ready(&messages) == messages);
        }
    }

    fn result(id: &str) -> Content {
        Content::ToolResult { id: id.into(), output: "r".into(), is_error: false }
    }
    fn call(id: &str) -> Content {
        Content::ToolUse { id: id.into(), name: "t".into(), input: serde_json::Value::Null }
    }
    fn says(role: Role, content: Vec<Content>) -> Message {
        Message { role, content }
    }

    #[test]
    fn a_broken_history_becomes_one_a_provider_takes() {
        let text = |t: &str| Content::Text { text: t.into() };
        let ready = provider_ready(&[
            says(Role::User, vec![result("before-the-cut"), text("hello")]),
            says(Role::User, vec![text("and")]),
            says(Role::Assistant, vec![call("a"), call("b")]),
            says(Role::User, vec![text("typed first"), result("b")]),
            says(Role::Assistant, vec![call("c")]),
            says(Role::Assistant, vec![text("no answer to c came")]),
        ]);
        assert_eq!(ready[0], says(Role::User, vec![text("hello"), text("and")]), "the orphan is out, and the two are one");
        let Content::ToolResult { id, is_error, .. } = &ready[2].content[0] else { panic!("{:?}", ready[2]) };
        assert_eq!((id.as_str(), *is_error), ("a", true), "an unanswered call is answered");
        assert_eq!(ready[2].content[1], result("b"));
        assert_eq!(ready[2].content[2], text("typed first"), "results first");
        assert!(matches!(&ready[4].content[0], Content::ToolResult { id, .. } if id == "c"));
        assert_eq!(ready[5].role, Role::Assistant);
        assert!(ready.windows(2).all(|w| w[0].role != w[1].role), "{ready:?}");
    }

    #[test]
    fn a_good_history_is_left_as_it_is() {
        let good = vec![says(Role::User, vec![Content::Text { text: "x".into() }]), says(Role::Assistant, vec![call("a")]), says(Role::User, vec![result("a")])];
        assert_eq!(provider_ready(&good), good);
    }
}
