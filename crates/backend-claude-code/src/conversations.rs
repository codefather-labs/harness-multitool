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
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// The same, reading a given environment: a window's, in the daemon.
    pub fn from_lookup(env: &dyn Fn(&str) -> Option<String>) -> PluginResult<Self> {
        if let Some(dir) = env("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
            return Ok(Self::new(dir));
        }
        let home = env("HOME").or_else(|| env("USERPROFILE")).ok_or_else(|| PluginError::new("no home directory"))?;
        Ok(Self::new(Path::new(&home).join(".claude")))
    }

    fn file(&self, root: &str, id: &str) -> PluginResult<PathBuf> {
        if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
            return Err(PluginError::new(format!("not a conversation id: {id}")));
        }
        Ok(self.folder(root).join(format!("{id}.jsonl")))
    }

    fn folder(&self, root: &str) -> PathBuf {
        let slug: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        self.dir.join("projects").join(slug)
    }
}

impl ConversationFormat for ClaudeCodeConversations {
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
            out.push(ConversationInfo { id: id.to_string(), title: title_near_end(&path).or_else(|| first_words(&path)).unwrap_or_default(), last_ms, bytes: meta.len() });
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
        entries.push(LogEntry { seq: entries.len() as u64, at_ms: last_ms, event: Event::FormSynced { source: NAME.to_string(), id: id.to_string(), cursor: cursor_at(&path, &nodes), holds: None } });
        Ok(ImportedConversation { info: ConversationInfo { id: id.to_string(), title, last_ms, bytes }, entries, damaged })
    }

    fn write(&self, root: &str, title: &str, history: &[Message]) -> PluginResult<String> {
        let folder = self.folder(root);
        std::fs::create_dir_all(&folder).map_err(|e| PluginError::new(format!("{}: {e}", folder.display())))?;
        let id = uuid();
        let path = folder.join(format!("{id}.jsonl"));
        let staging = folder.join(format!("{id}.partial"));
        std::fs::write(&staging, transcript(&id, root, title, history)).map_err(|e| PluginError::new(format!("{}: {e}", staging.display())))?;
        // Whole or not at all: Claude Code lists what it finds.
        std::fs::rename(&staging, &path).map_err(|e| PluginError::new(format!("{}: {e}", path.display())))?;
        Ok(id)
    }

    fn tip(&self, root: &str, id: &str) -> PluginResult<String> {
        let path = self.file(root, id)?;
        // Nothing yet: it stands at the start of what will be there.
        if !path.exists() {
            return Ok("0:".to_string());
        }
        let file = File::open(&path).map_err(|e| PluginError::new(format!("{}: {e}", path.display())))?;
        let (nodes, _, _) = read_nodes(file);
        Ok(cursor_at(&path, &nodes))
    }

    fn tip_after(&self, root: &str, id: &str, cursor: &str) -> PluginResult<String> {
        let path = self.file(root, id)?;
        if !path.exists() {
            return Ok(cursor.to_string());
        }
        let (bytes, leaf) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        let end = complete_length(&path).map_err(error)?;
        let (nodes, _, _) = read_nodes_range(&path, bytes, end).map_err(error)?;
        // The last line of the chain among the new ones; with none, the position stands.
        let tip = nodes.last().map_or(leaf, |n| n.uuid.clone());
        Ok(format!("{end}:{tip}"))
    }

    fn pull(&self, root: &str, id: &str, cursor: &str) -> PluginResult<PulledForm> {
        let path = self.file(root, id)?;
        if !path.exists() {
            return Ok(PulledForm { entries: Vec::new(), cursor: cursor.to_string(), rewound: false });
        }
        let (bytes, leaf) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        let end = complete_length(&path).map_err(error)?;
        // Nothing past the position: the usual case, and it does not read the file.
        if end == bytes {
            return Ok(PulledForm { entries: Vec::new(), cursor: cursor.to_string(), rewound: false });
        }
        // A long conversation runs to hundreds of megabytes: only what is new is read, unless the
        // new lines do not lead back to the position.
        if end > bytes {
            let (nodes, _, _) = read_nodes_range(&path, bytes, end).map_err(error)?;
            let tip = nodes.last().map_or(leaf.as_str(), |n| n.uuid.as_str());
            if nodes.iter().all(|n| n.value.is_none()) {
                return Ok(PulledForm { entries: Vec::new(), cursor: format!("{end}:{tip}"), rewound: false });
            }
            if let Some(rest) = chain_back_to(&nodes, &leaf) {
                return Ok(PulledForm { entries: to_entries(&nodes, &rest), cursor: format!("{end}:{tip}"), rewound: false });
            }
        }
        let file = File::open(&path).map_err(error)?;
        let (nodes, _, _) = read_nodes(file);
        let chain = chain(&nodes);
        let now = cursor_at(&path, &nodes);
        let (rest, rewound) = match chain.iter().rposition(|&i| nodes[i].uuid == leaf) {
            Some(at) => (&chain[at + 1..], false),
            None => {
                // Where the new branch left what was known: the position's own ancestors.
                let known: HashSet<usize> = nodes.iter().rposition(|n| n.uuid == leaf).map(|from| chain_from(&nodes, from)).unwrap_or_default().into_iter().collect();
                let at = chain.iter().rposition(|i| known.contains(i));
                // Only what was said counts: lines of its own that fell off the chain are no rewind.
                let dropped = known.iter().any(|i| nodes[*i].value.is_some() && !chain.contains(i));
                (&chain[at.map_or(0, |a| a + 1)..], dropped)
            }
        };
        Ok(PulledForm { entries: to_entries(&nodes, rest), cursor: now, rewound })
    }

    fn append(&self, root: &str, id: &str, cursor: &str, history: &[Message]) -> PluginResult<String> {
        let path = self.file(root, id)?;
        let (bytes, leaf) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        if complete_length(&path).map_err(error)? != bytes {
            return Err(PluginError::new(format!("{NAME} conversation {id} moved past the position it was to go on from")));
        }
        let (lines, tip) = lines_of(id, root, Some(leaf), history);
        // One write of whole lines at the end: every byte there stays as it was.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).map_err(error)?;
        std::io::Write::write_all(&mut file, lines.as_bytes()).map_err(error)?;
        Ok(format!("{}:{}", bytes + lines.len() as u64, tip.unwrap_or_default()))
    }

    fn held_by(&self, id: &str, except: Option<u32>) -> Option<String> {
        let files = std::fs::read_dir(self.dir.join("sessions")).ok()?;
        for file in files.flatten() {
            let path = file.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            let Ok(value) = std::fs::read_to_string(&path).map_err(|_| ()).and_then(|t| serde_json::from_str::<Value>(&t).map_err(|_| ())) else { continue };
            if value["sessionId"] == id && let Some(pid) = value["pid"].as_u64() && Some(pid) != except.map(u64::from) && process_lives(pid) {
                // A program run for a session, as a daemon's that has just been replaced runs its
                // own, goes within moments of losing its input.
                if value["entrypoint"] == "sdk-cli" {
                    let gone = (0..30).any(|_| {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        !process_lives(pid)
                    });
                    if gone {
                        continue;
                    }
                }
                return Some(format!("Claude Code (process {pid})"));
            }
        }
        None
    }
}

fn process_lives(pid: u64) -> bool {
    #[cfg(unix)]
    let answer = std::process::Command::new("kill").args(["-0", &pid.to_string()]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    #[cfg(not(unix))]
    let answer = std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()));
    answer
}

/// A position in a transcript: the bytes of whole lines read, and the last line of its chain.
fn parse_cursor(cursor: &str) -> PluginResult<(u64, String)> {
    let (bytes, leaf) = cursor.split_once(':').ok_or_else(|| PluginError::new(format!("not a position: {cursor}")))?;
    Ok((bytes.parse().map_err(|_| PluginError::new(format!("not a position: {cursor}")))?, leaf.to_string()))
}

/// The file's length up to its last whole line: a program may be in the middle of writing one.
fn complete_length(path: &Path) -> std::io::Result<u64> {
    let mut file = File::open(path)?;
    let mut end = file.metadata()?.len();
    let mut chunk = vec![0u8; 65536];
    while end > 0 {
        let start = end.saturating_sub(chunk.len() as u64);
        file.seek(SeekFrom::Start(start))?;
        let part = &mut chunk[..(end - start) as usize];
        std::io::Read::read_exact(&mut file, part)?;
        if let Some(at) = part.iter().rposition(|b| *b == b'\n') {
            return Ok(start + at as u64 + 1);
        }
        end = start;
    }
    Ok(0)
}

/// The last line of the file that has a place in its tree: what its next line goes on from.
fn cursor_at(path: &Path, nodes: &[Node]) -> String {
    let leaf = nodes.last().map_or("", |n| n.uuid.as_str());
    format!("{}:{leaf}", complete_length(path).unwrap_or(0))
}

/// A random id in the form Claude Code's sessions and lines use.
fn uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let part = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    let (a, b) = (part(), part());
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, 0x8000 | ((b >> 48) & 0x3fff), b & 0xffff_ffff_ffff)
}

/// The history as a transcript Claude Code resumes. Its reasoning blocks stay out: they are signed
/// for the conversation they were made in, and another provider's mean nothing to it.
fn transcript(id: &str, root: &str, title: &str, history: &[Message]) -> String {
    let mut out = json!({"type": "custom-title", "customTitle": title, "sessionId": id}).to_string();
    out.push('\n');
    out.push_str(&lines_of(id, root, None, history).0);
    out
}

/// A history as lines of a transcript, going on from `parent`, and the last of them.
fn lines_of(id: &str, root: &str, mut parent: Option<String>, history: &[Message]) -> (String, Option<String>) {
    // The time it is: Claude Code goes on from the latest branch of a file.
    let stamp = rfc3339_now();
    let mut out = String::new();
    for message in history {
        let uuid = uuid();
        let content: Vec<Value> = message.content.iter().filter_map(|c| match c {
            Content::Text { text } => Some(json!({"type": "text", "text": text})),
            Content::ToolUse { id, name, input } => Some(json!({"type": "tool_use", "id": id, "name": name, "input": input})),
            Content::ToolResult { id, output, is_error } => Some(json!({"type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error})),
            Content::Opaque { .. } => None,
        }).collect();
        let body = match message.role {
            Role::User => json!({"role": "user", "content": content}),
            Role::Assistant => json!({"id": format!("msg_{}", uuid.replace('-', "")), "type": "message", "role": "assistant", "model": "harness-import", "content": content, "stop_reason": "end_turn", "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}),
        };
        let kind = if message.role == Role::User { "user" } else { "assistant" };
        let line = json!({"type": kind, "uuid": uuid, "parentUuid": parent, "isSidechain": false, "userType": "external", "cwd": root, "sessionId": id, "version": "harness", "timestamp": stamp, "message": body});
        out.push_str(&line.to_string());
        out.push('\n');
        parent = Some(uuid);
    }
    (out, parent)
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

/// What the user first said, for a conversation that was never given a title.
fn first_words(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(std::io::Read::take(file, TITLE_TAIL)).split(b'\n').map_while(Result::ok) {
        let Ok(value) = serde_json::from_slice::<Value>(&line) else { continue };
        if value["type"] != "user" || value["isMeta"] == true {
            continue;
        }
        let content = &value["message"]["content"];
        let text = content.as_str().map(str::to_string).or_else(|| content.as_array()?.iter().find_map(|b| b["text"].as_str().map(str::to_string)));
        if let Some(words) = text.as_deref().and_then(|t| t.lines().next()).filter(|t| !t.trim().is_empty() && !t.starts_with('<')) {
            return Some(words.to_string());
        }
    }
    None
}

/// One line that has a place in the chain; `value` only for what will be shown, made small.
struct Node {
    uuid: String,
    parent: Option<String>,
    logical: Option<String>,
    value: Option<Value>,
}

fn read_nodes(file: File) -> (Vec<Node>, Option<String>, usize) {
    read_lines(BufReader::new(file))
}

/// The lines of a part of the file, between two ends of whole lines.
fn read_nodes_range(path: &Path, from: u64, to: u64) -> std::io::Result<(Vec<Node>, Option<String>, usize)> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    Ok(read_lines(BufReader::new(std::io::Read::take(file, to.saturating_sub(from)))))
}

fn read_lines(reader: impl BufRead) -> (Vec<Node>, Option<String>, usize) {
    let (mut nodes, mut custom, mut ai, mut damaged) = (Vec::new(), None, None, 0);
    for line in reader.split(b'\n').map_while(Result::ok) {
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
    nodes.iter().rposition(|n| n.value.is_some()).map_or(Vec::new(), |tip| chain_from(nodes, tip))
}

/// The chain of the new lines back to the line `leaf`, or none if they do not lead there.
fn chain_back_to(nodes: &[Node], leaf: &str) -> Option<Vec<usize>> {
    let at: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.uuid.as_str(), i)).collect();
    let mut here = nodes.iter().rposition(|n| n.value.is_some())?;
    let mut out = Vec::new();
    for _ in 0..=nodes.len() {
        out.push(here);
        let node = &nodes[here];
        let up = node.parent.as_deref().or(node.logical.as_deref())?;
        if up == leaf {
            out.reverse();
            return Some(out);
        }
        here = *at.get(up)?;
    }
    None
}

/// The lines leading to line `here`, in order.
fn chain_from(nodes: &[Node], mut here: usize) -> Vec<usize> {
    let at: HashMap<&str, usize> = nodes.iter().enumerate().map(|(i, n)| (n.uuid.as_str(), i)).collect();
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

/// Now, as Claude Code writes a time.
fn rfc3339_now() -> String {
    let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let (days, rest) = (millis.div_euclid(86_400_000), millis.rem_euclid(86_400_000));
    // The civil calendar from days, by Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z", rest / 3_600_000, rest / 60_000 % 60, rest / 1000 % 60, rest % 1000)
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
    // A call cut off by an interruption or a compaction has no result in the record: its card must
    // not stay running for ever, and the history must stay valid for a provider.
    let answer_rest = |events: &mut Vec<(u64, Event)>, at_ms: u64| {
        let back = events.iter().rev().map(|(_, e)| e).take_while(|e| !matches!(e, Event::UserMessage { .. } | Event::ContextSummary { .. }));
        let (mut calls, mut answered) = (Vec::new(), HashSet::new());
        for event in back {
            match event {
                Event::ModelToolCall { call } => calls.push(call.id.clone()),
                Event::ToolResult { id, .. } => {
                    answered.insert(id.clone());
                }
                _ => {}
            }
        }
        calls.retain(|id| !answered.contains(id));
        calls.reverse();
        events.extend(calls.into_iter().map(|id| (at_ms, Event::ToolResult { id, output: "no result in the record".to_string(), is_error: true })));
    };
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
                    answer_rest(&mut events, at_ms);
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
                    answer_rest(&mut events, at_ms);
                    events.push((at_ms, Event::UserMessage { text }));
                } else if value["isCompactSummary"] == true {
                    answer_rest(&mut events, at_ms);
                    events.push((at_ms, Event::ContextSummary { text: crate::result_text(&value["message"]["content"]) }));
                }
            }
            _ => {
                // A boundary: its summary follows in the next line.
                finish(&mut open, turn, at_ms, &mut events);
            }
        }
    }
    finish(&mut open, turn, at_ms, &mut events);
    answer_rest(&mut events, at_ms);
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
        assert!(c.entries.iter().any(|e| matches!(&e.event, Event::ContextSummary { text } if text == "the summary")));
        let state = SessionState::fold(c.entries.iter().map(|e| &e.event));
        assert_eq!(state.compacted_upto, 1, "what precedes the summary is not sent");
        assert!(!state.backend_fork, "linked, not forked");
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
            Event::UserMessage { .. } => "user", Event::TurnStarted { .. } => "start", Event::TextDelta { .. } => "text", Event::ModelToolCall { .. } => "model-call", Event::ToolRequested { .. } => "card",
            Event::TurnFinished { .. } => "finish", Event::ToolResult { .. } => "result", Event::ConversationImported { .. } => "imported", Event::FormSynced { .. } => "form", _ => "other",
        }).collect();
        assert_eq!(kinds, ["user", "start", "text", "model-call", "card", "finish", "result", "user", "imported", "form"]);
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

    /// The log is the format: what came from one provider's form goes back to it whole.
    #[test]
    fn a_history_written_reads_back_and_is_valid_for_a_provider() {
        let text = |t: &str| Content::Text { text: t.to_string() };
        let history = vec![
            Message { role: Role::User, content: vec![text("first")] },
            Message { role: Role::Assistant, content: vec![text("running"), Content::Opaque { block: json!({"type": "thinking", "signature": "other's"}) }, Content::ToolUse { id: "t1".into(), name: "Bash".into(), input: json!({"command": "ls"}) }] },
            Message { role: Role::User, content: vec![Content::ToolResult { id: "t1".into(), output: "a.txt".into(), is_error: false }] },
            Message { role: Role::Assistant, content: vec![text("done")] },
        ];
        let dir = tempfile::tempdir().unwrap();
        let source = ClaudeCodeConversations::new(dir.path());
        let id = source.write("/work/p", "my talk", &history).unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(id.as_bytes()[14], b'4', "a version 4 uuid: {id}");
        let back = source.read("/work/p", &id).unwrap();
        assert_eq!(back.info.title, "my talk");
        assert!(!format!("{:?}", back.entries).contains("other's"), "a reasoning block stays out");
        let state = SessionState::fold(back.entries.iter().map(|e| &e.event));
        let mut expected = history.clone();
        expected[1].content.remove(1);
        assert_eq!(provider_ready(&state.messages), expected);
        assert_eq!(source.list("/work/p").unwrap()[0].id, id);
    }

    #[test]
    fn it_ends_by_naming_the_conversation_and_the_time_is_the_conversations() {
        let c = import(&[user("u1", None, "first"), r#"{"type":"ai-title","aiTitle":"a"}"#.to_string(), r#"{"type":"custom-title","customTitle":"mine"}"#.to_string(), r#"{"type":"ai-title","aiTitle":"b"}"#.to_string()]);
        assert_eq!(c.info.title, "mine");
        assert!(matches!(&c.entries[c.entries.len() - 2].event, Event::ConversationImported { id, source, .. } if id == "abc" && source == NAME));
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

    /// INV-20, INV-21: a form and a log kept in line by position: lines appended on the chain, every
    /// byte before them as it was; what the form gained read once; a half-written line waits.
    #[test]
    fn a_form_is_appended_to_and_pulled_from_by_position() {
        let dir = std::env::temp_dir().join(format!("mt-form-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let format = ClaudeCodeConversations::new(&dir);
        let root = "/work";
        let id = format.write(root, "t", &[Message::user("one".to_string())]).unwrap();
        let cursor = format.tip(root, &id).unwrap();
        assert!(format.pull(root, &id, &cursor).unwrap().entries.is_empty());
        let path = format.file(root, &id).unwrap();
        let before = std::fs::read(&path).unwrap();

        let second = format.append(root, &id, &cursor, &[Message { role: Role::Assistant, content: vec![Content::Text { text: "two".into() }] }]).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[..before.len()], &before[..], "only appended");
        assert!(format.pull(root, &id, &second).unwrap().entries.is_empty(), "its own lines do not come back");
        assert!(format.append(root, &id, &cursor, &[Message::user("x".to_string())]).is_err(), "not from an old position");
        let whole = format.read(root, &id).unwrap();
        let said = |entries: &[LogEntry]| entries.iter().filter_map(|e| match &e.event {
            Event::UserMessage { text } => Some(text.clone()),
            Event::TextDelta { text } => Some(text.clone()),
            _ => None,
        }).collect::<Vec<_>>();
        assert_eq!(said(&whole.entries), ["one", "two"]);

        // The provider's program writes on: a turn, and a line it has not finished.
        let leaf = second.split_once(':').unwrap().1.to_string();
        let outside = json!({"type": "user", "uuid": "out-1", "parentUuid": leaf, "isSidechain": false, "timestamp": "2026-11-01T10:00:00.500Z", "sessionId": id, "message": {"role": "user", "content": "three"}});
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{outside}").unwrap();
        write!(file, "{{\"type\":\"user\",\"uuid\":\"half").unwrap();
        let pulled = format.pull(root, &id, &second).unwrap();
        assert_eq!(said(&pulled.entries), ["three"]);
        assert!(!pulled.rewound);
        assert!(pulled.cursor.ends_with(":out-1"));
        assert!(format.pull(root, &id, &pulled.cursor).unwrap().entries.is_empty(), "read once; the half line waits");
        assert_eq!(format.tip_after(root, &id, &second).unwrap(), pulled.cursor);

        // A rewind: the new line goes on from the first one.
        let first = whole.entries.len();
        assert!(first > 0);
        let start = std::fs::read_to_string(&path).unwrap().lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find_map(|v| v["uuid"].as_str().map(str::to_string)).unwrap();
        writeln!(file, "\"}}").unwrap();
        let branch = json!({"type": "user", "uuid": "branch-1", "parentUuid": start, "isSidechain": false, "timestamp": "2026-11-01T10:00:00.500Z", "sessionId": id, "message": {"role": "user", "content": "other way"}});
        writeln!(file, "{branch}").unwrap();
        let rewound = format.pull(root, &id, &pulled.cursor).unwrap();
        assert!(rewound.rewound);
        assert_eq!(said(&rewound.entries), ["other way"]);
    }

    /// CON-78: a live program named in the provider's record holds a conversation; a dead one does not.
    #[test]
    fn a_conversation_is_held_only_by_a_program_alive() {
        let dir = std::env::temp_dir().join(format!("mt-held-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sessions")).unwrap();
        let format = ClaudeCodeConversations::new(&dir);
        assert_eq!(format.held_by("c1", None), None);
        std::fs::write(dir.join("sessions/1.json"), json!({"pid": std::process::id(), "sessionId": "c1"}).to_string()).unwrap();
        std::fs::write(dir.join("sessions/2.json"), json!({"pid": 4_000_000_000u64, "sessionId": "c2"}).to_string()).unwrap();
        assert!(format.held_by("c1", None).unwrap().contains("Claude Code"));
        assert_eq!(format.held_by("c2", None), None);
        assert_eq!(format.held_by("c3", None), None);
        assert_eq!(format.held_by("c1", Some(std::process::id())), None, "its own process does not count");
    }
}
