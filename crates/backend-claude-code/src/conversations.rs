//! IF-18 for Claude Code (ADR 0022): reads a conversation it keeps, for the harness's screen. Only
//! reads: the conversation goes on in Claude Code itself, in a fork. The format is Claude Code's
//! own and undocumented, so anything unknown is passed over and damage is counted, not fatal.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use harness_api::event::{Event, LogEntry};
use harness_api::*;
use serde_json::{Value, json};

use crate::{Mapped, Mapper, NAME};

/// What one tool result keeps in the harness's log; the whole stays in Claude Code's own record.
const OUTPUT_CAP: usize = 4096;
/// Where a listing looks for a title: it is written again and again, so it is near the end.
const TITLE_TAIL: u64 = 1 << 20;

pub struct ClaudeCodeConversations {
    dir: PathBuf,
}

impl ClaudeCodeConversations {
    /// `dir` is Claude Code's configuration directory.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        ClaudeCodeConversations { dir: dir.into() }
    }

    /// `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub fn from_env() -> PluginResult<Self> {
        if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return Ok(Self::new(dir));
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).ok_or_else(|| PluginError::new("no home directory"))?;
        Ok(Self::new(Path::new(&home).join(".claude")))
    }

    fn folder(&self, root: &str) -> PathBuf {
        let slug: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        self.dir.join("projects").join(slug)
    }
}

impl ConversationSource for ClaudeCodeConversations {
    fn name(&self) -> &str {
        NAME
    }

    fn list(&self, root: &str) -> PluginResult<Vec<ConversationInfo>> {
        let folder = self.folder(root);
        let Ok(files) = std::fs::read_dir(&folder) else { return Ok(Vec::new()) };
        let mut out = Vec::new();
        for file in files.flatten() {
            let path = file.path();
            let (Some(id), Ok(meta)) = (path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".jsonl")), file.metadata()) else { continue };
            let last_ms = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            out.push(ConversationInfo { id: id.to_string(), title: title_near_end(&path).unwrap_or_default(), last_ms, bytes: meta.len() });
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.last_ms));
        Ok(out)
    }

    fn read(&self, root: &str, id: &str) -> PluginResult<ImportedConversation> {
        if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
            return Err(PluginError::new(format!("not a conversation id: {id}")));
        }
        let folder = self.folder(root);
        let path = folder.join(format!("{id}.jsonl"));
        let file = File::open(&path).map_err(|e| PluginError::new(format!("no conversation {id} for this directory, in {}: {e}. Claude Code resumes a conversation only from the directory it began in.", folder.display())))?;
        let bytes = file.metadata().map_or(0, |m| m.len());
        let (nodes, titles, damaged) = read_nodes(file);
        let chain = chain(&nodes);
        let mut entries = to_entries(&nodes, &chain);
        let title = titles.or_else(|| entries.iter().find_map(|e| match &e.event {
            Event::UserMessage { text } => text.lines().next().map(str::to_string),
            _ => None,
        })).unwrap_or_else(|| id.to_string());
        let last_ms = entries.last().map_or(0, |e| e.at_ms);
        entries.push(LogEntry { seq: entries.len() as u64, at_ms: last_ms, event: Event::ConversationImported { source: NAME.to_string(), id: id.to_string(), title: title.clone() } });
        Ok(ImportedConversation { info: ConversationInfo { id: id.to_string(), title, last_ms, bytes }, entries, damaged })
    }
}

/// What the file says of a title, the last one written: a name the user gave wins.
fn title_of(line: &Value, custom: &mut Option<String>, ai: &mut Option<String>) {
    match line["type"].as_str() {
        Some("custom-title") => *custom = line["customTitle"].as_str().map(str::to_string),
        Some("ai-title") => *ai = line["aiTitle"].as_str().map(str::to_string),
        _ => {}
    }
}

fn title_near_end(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(TITLE_TAIL))).ok()?;
    let (mut custom, mut ai) = (None, None);
    for line in BufReader::new(file).split(b'\n').map_while(Result::ok) {
        if line.len() < 4096 && let Ok(value) = serde_json::from_slice::<Value>(&line) {
            title_of(&value, &mut custom, &mut ai);
        }
    }
    custom.or(ai)
}

/// One line that has a place in the chain; `value` only for what will be shown, made small.
struct Node {
    uuid: String,
    parent: Option<String>,
    logical: Option<String>,
    value: Option<Value>,
}

fn read_nodes(file: File) -> (Vec<Node>, Option<String>, usize) {
    let (mut nodes, mut custom, mut ai, mut damaged) = (Vec::new(), None, None, 0);
    for line in BufReader::new(file).split(b'\n').map_while(Result::ok) {
        if line.is_empty() {
            continue;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(&line) else {
            damaged += 1;
            continue;
        };
        title_of(&value, &mut custom, &mut ai);
        let Some(uuid) = value["uuid"].as_str().map(str::to_string) else { continue };
        if value["isSidechain"] == true {
            continue;
        }
        let parent = value["parentUuid"].as_str().map(str::to_string);
        let logical = value["logicalParentUuid"].as_str().map(str::to_string);
        let shown = matches!(value["type"].as_str(), Some("user" | "assistant")) || value["subtype"] == "compact_boundary";
        if shown {
            shrink(&mut value);
        }
        nodes.push(Node { uuid, parent, logical, value: shown.then_some(value) });
    }
    (nodes, custom.or(ai), damaged)
}

/// Images out and tool results cut, as each line is read: this is what keeps a long conversation
/// in memory. What the line says of itself and its message stays.
fn shrink(value: &mut Value) {
    let Some(content) = value["message"]["content"].as_array_mut() else { return };
    for block in content {
        match block["type"].as_str() {
            Some("image") => *block = json!({"type": "text", "text": "[image]"}),
            Some("tool_result") => block["content"] = Value::String(cap(&crate::result_text(&block["content"]))),
            Some("thinking") => *block = Value::Null,
            _ => {}
        }
    }
}

fn cap(text: &str) -> String {
    if text.len() <= OUTPUT_CAP {
        return text.to_string();
    }
    let mut end = OUTPUT_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… cut on import, {} bytes in all; Claude Code keeps the whole of it", &text[..end], text.len())
}

/// The conversation as it stood, from the last line back: a rewound branch is in the file and not
/// here. A compaction starts a chain over and names the line before it. Where a parent is missing,
/// as a damaged line leaves it, the line before in the file takes over.
fn chain(nodes: &[Node]) -> Vec<usize> {
    let at: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.uuid.as_str(), i)).collect();
    let Some(mut here) = nodes.iter().rposition(|n| n.value.is_some()) else { return Vec::new() };
    let (mut out, mut visited) = (Vec::new(), HashSet::new());
    loop {
        if !visited.insert(here) {
            break;
        }
        out.push(here);
        let node = &nodes[here];
        let up = node.parent.as_deref().or(node.logical.as_deref());
        here = match up {
            Some(uuid) => match at.get(uuid) {
                Some(&next) => next,
                None if here > 0 => here - 1,
                None => break,
            },
            None => break,
        };
    }
    out.reverse();
    out
}

/// The time of an RFC 3339 stamp as Claude Code writes it, in UTC.
fn millis(stamp: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let fraction = stamp.get(20..23).and_then(|f| f.parse::<i64>().ok()).unwrap_or(0);
    // Days from the civil calendar, by Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from((days * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + fraction).ok()
}

/// What the user said in this line, if they did. Claude Code's own wrapping is not their words.
fn user_text(value: &Value) -> Option<String> {
    if value["isMeta"] == true || value["isCompactSummary"] == true || value["isVisibleInTranscriptOnly"] == true {
        return None;
    }
    let content = &value["message"]["content"];
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("<command-name>") {
        return rest.split("</command-name>").next().map(str::to_string);
    }
    let wrapped = ["<local-command", "<system-reminder>", "<task-notification>", "<command-message>"].iter().any(|w| text.starts_with(w));
    (!text.is_empty() && !wrapped).then(|| text.to_string())
}

fn usage_of(message: &Value) -> Usage {
    let count = |k: &str| message["usage"][k].as_u64().unwrap_or(0);
    Usage { input: count("input_tokens"), output: count("output_tokens"), cache_read: count("cache_read_input_tokens"), cache_write: count("cache_creation_input_tokens") }
}

fn stop_of(message: &Value) -> StopReason {
    match message["stop_reason"].as_str() {
        Some("tool_use") => StopReason::ToolUse,
        Some("max_tokens") => StopReason::MaxTokens,
        Some("end_turn") | None => StopReason::EndTurn,
        Some(other) => StopReason::Other(other.to_string()),
    }
}

fn to_entries(nodes: &[Node], chain: &[usize]) -> Vec<LogEntry> {
    let mut mapper = Mapper::new(1);
    let mut events: Vec<(u64, Event)> = Vec::new();
    let (mut at_ms, mut turn) = (0, 0);
    // The message whose lines are coming: its id, and the line that says how it ended.
    let mut open: Option<(String, Value)> = None;
    let finish = |open: &mut Option<(String, Value)>, turn: u32, at_ms: u64, events: &mut Vec<(u64, Event)>| {
        if let Some((_, message)) = open.take() {
            events.push((at_ms, Event::TurnFinished { turn, stop: stop_of(&message), usage: usage_of(&message), cost_micro_usd: None }));
        }
    };
    for value in chain.iter().filter_map(|&i| nodes[i].value.as_ref()) {
        at_ms = value["timestamp"].as_str().and_then(millis).unwrap_or(at_ms);
        match value["type"].as_str() {
            Some("assistant") => {
                let id = value["message"]["id"].as_str().unwrap_or_default().to_string();
                if open.as_ref().is_none_or(|(current, _)| *current != id) {
                    finish(&mut open, turn, at_ms, &mut events);
                    turn += 1;
                    events.push((at_ms, Event::TurnStarted { turn, provider: NAME.to_string(), model: value["message"]["model"].as_str().unwrap_or_default().to_string() }));
                }
                open = Some((id, value["message"].clone()));
                for mapped in mapper.line(&value.to_string()) {
                    if let Mapped::Event(event) = mapped {
                        events.push((at_ms, event));
                    }
                }
            }
            Some("user") => {
                finish(&mut open, turn, at_ms, &mut events);
                for mapped in mapper.line(&value.to_string()) {
                    if let Mapped::Event(event) = mapped {
                        events.push((at_ms, event));
                    }
                }
                if let Some(text) = user_text(value) {
                    events.push((at_ms, Event::UserMessage { text }));
                }
            }
            _ => {
                finish(&mut open, turn, at_ms, &mut events);
                let tokens = |k: &str| value["compactMetadata"][k].as_u64().unwrap_or(0);
                events.push((at_ms, Event::Compacted { upto: 0, tokens_before: tokens("preTokens"), tokens_after: tokens("postTokens") }));
            }
        }
    }
    finish(&mut open, turn, at_ms, &mut events);
    // A call cut off by an interruption or a compaction has no result in the record: its card must
    // not stay running for ever.
    let answered: HashSet<String> = events.iter().filter_map(|(_, e)| if let Event::ToolResult { id, .. } = e { Some(id.clone()) } else { None }).collect();
    let unanswered: Vec<String> = events.iter().filter_map(|(_, e)| if let Event::ToolRequested { call, .. } = e { (!answered.contains(&call.id)).then(|| call.id.clone()) } else { None }).collect();
    events.extend(unanswered.into_iter().map(|id| (at_ms, Event::ToolResult { id, output: "no result in the record".to_string(), is_error: true })));
    events.into_iter().enumerate().map(|(seq, (at_ms, event))| LogEntry { seq: seq as u64, at_ms, event }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(uuid: &str, parent: Option<&str>, rest: Value) -> String {
        let mut v = json!({"uuid": uuid, "parentUuid": parent, "timestamp": "2026-11-01T10:00:00.500Z", "sessionId": "s"});
        v.as_object_mut().unwrap().extend(rest.as_object().unwrap().clone());
        v.to_string()
    }
    fn user(uuid: &str, parent: Option<&str>, text: &str) -> String {
        line(uuid, parent, json!({"type": "user", "message": {"role": "user", "content": text}}))
    }
    fn assistant(uuid: &str, parent: &str, id: &str, block: Value) -> String {
        line(uuid, Some(parent), json!({"type": "assistant", "message": {"id": id, "model": "m1", "role": "assistant", "stop_reason": "end_turn", "usage": {"input_tokens": 3, "output_tokens": 4}, "content": [block]}}))
    }
    fn import(lines: &[String]) -> ImportedConversation {
        let dir = tempfile::tempdir().unwrap();
        let root = "/work/my_project";
        let folder = dir.path().join("projects").join("-work-my-project");
        std::fs::create_dir_all(&folder).unwrap();
        let mut file = File::create(folder.join("abc.jsonl")).unwrap();
        file.write_all(lines.join("\n").as_bytes()).unwrap();
        ClaudeCodeConversations::new(dir.path()).read(root, "abc").unwrap()
    }
    fn users(c: &ImportedConversation) -> Vec<String> {
        c.entries.iter().filter_map(|e| if let Event::UserMessage { text } = &e.event { Some(text.clone()) } else { None }).collect()
    }

    #[test]
    fn the_conversation_is_its_chain_not_the_file() {
        let c = import(&[
            user("u1", None, "first"),
            assistant("a1", "u1", "m1", json!({"type": "text", "text": "reply"})),
            user("gone", Some("a1"), "a rewound branch"),
            assistant("gone2", "gone", "m2", json!({"type": "text", "text": "rewound reply"})),
            line("side", Some("a1"), json!({"type": "user", "isSidechain": true, "message": {"content": "a subagent's"}})),
            line("meta", Some("a1"), json!({"type": "user", "isMeta": true, "message": {"content": "not the user's words"}})),
            line("att", Some("meta"), json!({"type": "attachment", "attachment": {}})),
            user("u2", Some("att"), "second"),
        ]);
        assert_eq!(users(&c), ["first", "second"]);
        assert!(!format!("{:?}", c.entries).contains("rewound"));
    }

    #[test]
    fn a_compaction_is_crossed_and_its_summary_is_not_the_users() {
        let c = import(&[
            user("u1", None, "before"),
            line("b", None, json!({"type": "system", "subtype": "compact_boundary", "logicalParentUuid": "u1", "compactMetadata": {"preTokens": 900, "postTokens": 90}})),
            line("sum", Some("b"), json!({"type": "user", "isCompactSummary": true, "message": {"content": "the summary"}})),
            user("u2", Some("sum"), "after"),
        ]);
        assert_eq!(users(&c), ["before", "after"]);
        assert!(c.entries.iter().any(|e| matches!(e.event, Event::Compacted { tokens_before: 900, tokens_after: 90, .. })));
    }

    #[test]
    fn the_lines_of_one_message_are_one_turn_and_results_follow_their_calls() {
        let long = "ж".repeat(OUTPUT_CAP);
        let c = import(&[
            user("u1", None, "go"),
            assistant("a1", "u1", "m1", json!({"type": "thinking", "thinking": "private"})),
            assistant("a2", "a1", "m1", json!({"type": "text", "text": "running"})),
            assistant("a3", "a2", "m1", json!({"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "ls"}})),
            line("r1", Some("a3"), json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": long}]}})),
            line("i1", Some("r1"), json!({"type": "user", "message": {"content": [{"type": "text", "text": "look"}, {"type": "image", "source": {"data": "AAAA"}}]}})),
        ]);
        let kinds: Vec<&str> = c.entries.iter().map(|e| match &e.event {
            Event::UserMessage { .. } => "user", Event::TurnStarted { .. } => "start", Event::TextDelta { .. } => "text", Event::ToolRequested { .. } => "call",
            Event::TurnFinished { .. } => "finish", Event::ToolResult { .. } => "result", Event::ConversationImported { .. } => "imported", _ => "other",
        }).collect();
        assert_eq!(kinds, ["user", "start", "text", "call", "finish", "result", "user", "imported"]);
        let text = format!("{:?}", c.entries);
        assert!(!text.contains("private") && !text.contains("AAAA"));
        let output = c.entries.iter().find_map(|e| if let Event::ToolResult { output, .. } = &e.event { Some(output.clone()) } else { None }).unwrap();
        assert!(output.len() < OUTPUT_CAP + 100 && output.contains("cut on import"), "cut at a char boundary, and says so");
        assert!(matches!(&c.entries.iter().find(|e| matches!(e.event, Event::TurnStarted { .. })).unwrap().event, Event::TurnStarted { model, .. } if model == "m1"));
    }

    #[test]
    fn damage_is_counted_and_a_missing_parent_does_not_lose_the_past() {
        let c = import(&[user("u1", None, "first"), "\u{0}\u{0}\u{0}".to_string(), user("u2", Some("lost"), "second"), "{\"type\":\"user\",\"torn".to_string()]);
        assert_eq!(users(&c), ["first", "second"]);
        assert_eq!(c.damaged, 2);
    }

    #[test]
    fn a_call_with_no_result_does_not_stay_running() {
        let c = import(&[user("u1", None, "go"), assistant("a1", "u1", "m1", json!({"type": "tool_use", "id": "t1", "name": "Bash", "input": {}}))]);
        assert!(c.entries.iter().any(|e| matches!(&e.event, Event::ToolResult { id, is_error: true, .. } if id == "t1")));
    }

    #[test]
    fn it_ends_by_naming_the_conversation_and_the_time_is_the_conversations() {
        let c = import(&[user("u1", None, "first"), r#"{"type":"ai-title","aiTitle":"a"}"#.to_string(), r#"{"type":"custom-title","customTitle":"mine"}"#.to_string(), r#"{"type":"ai-title","aiTitle":"b"}"#.to_string()]);
        assert_eq!(c.info.title, "mine");
        assert!(matches!(&c.entries.last().unwrap().event, Event::ConversationImported { id, source, .. } if id == "abc" && source == NAME));
        assert_eq!(c.entries[0].at_ms, 1_793_527_200_500);
    }

    #[test]
    fn a_bad_id_or_another_directory_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let source = ClaudeCodeConversations::new(dir.path());
        assert!(source.read("/w", "../../secret").is_err());
        assert!(source.read("/w", "nope").unwrap_err().to_string().contains("directory it began in"));
        assert!(source.list("/w").unwrap().is_empty());
    }
}
