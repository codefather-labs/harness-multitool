//! Codex's form of a conversation (CON-134, journal#914; the format in journal#918): the rollout
//! files it keeps under `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl`, one JSON line
//! each: `{timestamp, ordinal, type, payload}`. The harness reads them into its log, writes a whole
//! conversation as one Codex resumes, appends what the log gained and pulls what Codex added. The
//! format is Codex's own and undocumented: anything unknown is passed over, damage is counted.
//!
//! A position is `bytes:ordinal`: the bytes of whole lines read, and the ordinal the next line
//! takes. Codex numbers every line it writes; a line the harness writes is numbered on from there.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use harness_api::event::{Event, LogEntry};
use harness_api::*;
use serde_json::{Value, json};

use crate::NAME;

/// What one tool result keeps in the harness's log; the whole stays in Codex's own record.
const OUTPUT_CAP: usize = 4096;
/// How much of a file a listing reads for its first words.
const HEAD: u64 = 1 << 20;
/// The version written into a session's first line: the one whose format journal#918 records.
const CLI_VERSION: &str = "0.166.0";

pub struct CodexConversations {
    dir: PathBuf,
    /// Where each conversation's file was found: finding one walks every day's folder.
    found: Mutex<HashMap<String, PathBuf>>,
}

impl CodexConversations {
    /// `dir` is Codex's folder (CODEX_HOME).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        CodexConversations { dir: dir.into(), found: Mutex::new(HashMap::new()) }
    }

    /// `$CODEX_HOME`, else `~/.codex`.
    pub fn from_env() -> PluginResult<Self> {
        Self::from_lookup(&|name| std::env::var(name).ok())
    }

    /// The same, reading a given environment: a window's, in the daemon.
    pub fn from_lookup(env: &dyn Fn(&str) -> Option<String>) -> PluginResult<Self> {
        if let Some(dir) = env("CODEX_HOME").filter(|d| !d.is_empty()) {
            return Ok(Self::new(dir));
        }
        let home = env("HOME").or_else(|| env("USERPROFILE")).ok_or_else(|| PluginError::new("no home directory"))?;
        Ok(Self::new(Path::new(&home).join(".codex")))
    }

    fn sessions(&self) -> PathBuf {
        self.dir.join("sessions")
    }

    /// The file of conversation `id`, if there is one.
    fn file(&self, id: &str) -> PluginResult<Option<PathBuf>> {
        if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
            return Err(PluginError::new(format!("not a conversation id: {id}")));
        }
        let mut found = self.found.lock().expect("found");
        if let Some(path) = found.get(id).filter(|p| p.exists()) {
            return Ok(Some(path.clone()));
        }
        let suffix = format!("-{id}.jsonl");
        let path = rollouts(&self.sessions()).into_iter().find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix)));
        if let Some(path) = &path {
            found.insert(id.to_string(), path.clone());
        }
        Ok(path)
    }

    fn existing(&self, id: &str) -> PluginResult<PathBuf> {
        self.file(id)?.ok_or_else(|| PluginError::new(format!("no {NAME} conversation {id} in {}", self.sessions().display())))
    }
}

/// Every rollout file under `dir`, at any depth.
fn rollouts(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(dir) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push(path);
            } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")) {
                out.push(path);
            }
        }
    }
    out
}

impl ConversationFormat for CodexConversations {
    fn name(&self) -> &str {
        NAME
    }

    fn list(&self, root: &str) -> PluginResult<Vec<ConversationInfo>> {
        let mut out = Vec::new();
        for path in rollouts(&self.sessions()) {
            let Some((id, cwd, title)) = head_of(&path) else { continue };
            if cwd != root {
                continue;
            }
            let meta = std::fs::metadata(&path).ok();
            let last_ms = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
            self.found.lock().expect("found").insert(id.clone(), path.clone());
            out.push(ConversationInfo { id, title: title.unwrap_or_default(), last_ms, bytes: meta.map_or(0, |m| m.len()) });
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.last_ms));
        Ok(out)
    }

    fn read(&self, _root: &str, id: &str) -> PluginResult<ImportedConversation> {
        let path = self.existing(id)?;
        let file = File::open(&path).map_err(|e| PluginError::new(format!("{}: {e}", path.display())))?;
        let bytes = file.metadata().map_or(0, |m| m.len());
        let lines = read_lines(BufReader::new(harness_api::platform::progress::Counting::new(file, "Reading Codex's copy", bytes)));
        let mut entries = to_entries(&lines.items);
        let title = entries.iter().find_map(|e| match &e.event {
            Event::UserMessage { text } => text.lines().next().map(str::to_string),
            _ => None,
        }).unwrap_or_else(|| id.to_string());
        let last_ms = entries.last().map_or(0, |e| e.at_ms);
        let cursor = format!("{}:{}", complete_length(&path).unwrap_or(0), lines.next_ordinal);
        entries.push(LogEntry { seq: entries.len() as u64, at_ms: last_ms, event: Event::ConversationImported { source: NAME.to_string(), id: id.to_string(), title: title.clone() } });
        entries.push(LogEntry { seq: entries.len() as u64, at_ms: last_ms, event: Event::FormSynced { source: NAME.to_string(), id: id.to_string(), cursor, holds: None, digest: None } });
        Ok(ImportedConversation { info: ConversationInfo { id: id.to_string(), title, last_ms, bytes }, entries, damaged: lines.damaged })
    }

    fn write(&self, root: &str, _title: &str, history: &[Message]) -> PluginResult<String> {
        let now = now_millis();
        let id = uuid_v7(now);
        let stamp = rfc3339(now);
        let folder = self.sessions().join(&stamp[0..4]).join(&stamp[5..7]).join(&stamp[8..10]);
        std::fs::create_dir_all(&folder).map_err(|e| PluginError::new(format!("{}: {e}", folder.display())))?;
        let name = format!("rollout-{}-{id}", stamp[..19].replace(':', "-"));
        let path = folder.join(format!("{name}.jsonl"));
        let staging = folder.join(format!("{name}.partial"));
        // The first line names the session; Codex resumes from that and the messages alone (journal#918).
        let meta = json!({"timestamp": stamp, "ordinal": 0, "type": "session_meta", "payload": {"id": id, "session_id": id, "timestamp": stamp, "cwd": root, "originator": "codex_exec", "cli_version": CLI_VERSION, "source": "exec", "model_provider": "openai"}});
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", staging.display()));
        let mut file = std::io::BufWriter::new(File::create(&staging).map_err(error)?);
        std::io::Write::write_all(&mut file, format!("{meta}\n").as_bytes()).map_err(error)?;
        write_lines(&mut file, 1, &stamp, history).map_err(error)?;
        std::io::Write::flush(&mut file).map_err(error)?;
        drop(file);
        // Whole or not at all: Codex lists what it finds.
        std::fs::rename(&staging, &path).map_err(|e| PluginError::new(format!("{}: {e}", path.display())))?;
        self.found.lock().expect("found").insert(id.clone(), path);
        Ok(id)
    }

    fn tip(&self, _root: &str, id: &str) -> PluginResult<String> {
        // Nothing yet: it stands at the start of what will be there.
        let Some(path) = self.file(id)? else { return Ok("0:0".to_string()) };
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        let end = complete_length(&path).map_err(error)?;
        let lines = read_range(&path, 0, end).map_err(error)?;
        Ok(format!("{end}:{}", lines.next_ordinal))
    }

    fn tip_after(&self, _root: &str, id: &str, cursor: &str) -> PluginResult<String> {
        let Some(path) = self.file(id)? else { return Ok(cursor.to_string()) };
        let (bytes, ordinal) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        let end = complete_length(&path).map_err(error)?;
        if end < bytes {
            return self.tip(_root, id);
        }
        let lines = read_range(&path, bytes, end).map_err(error)?;
        Ok(format!("{end}:{}", lines.next_ordinal.max(ordinal)))
    }

    fn pull(&self, _root: &str, id: &str, cursor: &str) -> PluginResult<PulledForm> {
        let Some(path) = self.file(id)? else { return Ok(PulledForm { entries: Vec::new(), cursor: cursor.to_string(), rewound: false }) };
        let (bytes, ordinal) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        let end = complete_length(&path).map_err(error)?;
        // Nothing past the position: the usual case, and it does not read the file.
        if end == bytes {
            return Ok(PulledForm { entries: Vec::new(), cursor: cursor.to_string(), rewound: false });
        }
        // Codex only ever appends: a shorter file was changed outside it, and what is left is not
        // new (CON-138 checks it against the log).
        if end < bytes {
            return Err(PluginError::new(format!("{NAME} conversation {id} is shorter than when the harness last read it: it was changed outside Codex")));
        }
        let lines = read_range(&path, bytes, end).map_err(error)?;
        Ok(PulledForm { entries: to_entries(&lines.items), cursor: format!("{end}:{}", lines.next_ordinal.max(ordinal)), rewound: false })
    }

    fn append(&self, _root: &str, id: &str, cursor: &str, history: &[Message]) -> PluginResult<String> {
        let path = self.existing(id)?;
        let (bytes, ordinal) = parse_cursor(cursor)?;
        let error = |e: std::io::Error| PluginError::new(format!("{}: {e}", path.display()));
        if complete_length(&path).map_err(error)? != bytes {
            return Err(PluginError::new(format!("{NAME} conversation {id} moved past the position it was to go on from")));
        }
        let (lines, next) = lines_of(ordinal, &rfc3339(now_millis()), history);
        // One write of whole lines at the end: every byte there stays as it was.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).map_err(error)?;
        std::io::Write::write_all(&mut file, lines.as_bytes()).map_err(error)?;
        Ok(format!("{}:{next}", bytes + lines.len() as u64))
    }

    fn exists(&self, _root: &str, id: &str) -> bool {
        self.file(id).is_ok_and(|p| p.is_some())
    }

    fn held_by(&self, id: &str, except: Option<u32>) -> Option<String> {
        // Codex keeps no record of its running processes: a codex whose command line names the
        // conversation (`codex exec resume <id>`, `codex resume <id>`) holds it.
        let pid = holder(id, except)?;
        Some(format!("Codex (process {pid})"))
    }
}

#[cfg(unix)]
fn holder(id: &str, except: Option<u32>) -> Option<u32> {
    let out = std::process::Command::new("ps").args(["-eo", "pid=,args="]).stderr(std::process::Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
        let (pid, args) = line.trim_start().split_once(' ')?;
        let pid: u32 = pid.parse().ok()?;
        let words: Vec<&str> = args.split_whitespace().collect();
        let codex = words.first().is_some_and(|w| Path::new(w).file_name().is_some_and(|n| n == "codex"))
            || words.iter().take(3).any(|w| w.ends_with("/codex") || w.ends_with("/codex.js"));
        (codex && Some(pid) != except && words.windows(2).any(|w| w[0] == "resume" && w[1] == id)).then_some(pid)
    })
}

#[cfg(not(unix))]
fn holder(_id: &str, _except: Option<u32>) -> Option<u32> {
    None
}

/// A position: the bytes of whole lines read, and the ordinal the next line takes.
fn parse_cursor(cursor: &str) -> PluginResult<(u64, u64)> {
    let bad = || PluginError::new(format!("not a position: {cursor}"));
    let (bytes, ordinal) = cursor.split_once(':').ok_or_else(bad)?;
    Ok((bytes.parse().map_err(|_| bad())?, if ordinal.is_empty() { 0 } else { ordinal.parse().map_err(|_| bad())? }))
}

/// The file's length up to its last whole line: Codex may be in the middle of writing one.
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

/// The id, the directory and the first words of a conversation, from the head of its file.
fn head_of(path: &Path) -> Option<(String, String, Option<String>)> {
    let file = File::open(path).ok()?;
    let mut lines = BufReader::new(std::io::Read::take(file, HEAD)).split(b'\n').map_while(Result::ok);
    let first: Value = serde_json::from_slice(&lines.next()?).ok()?;
    if first["type"] != "session_meta" {
        return None;
    }
    let meta = &first["payload"];
    let id = meta["id"].as_str()?.to_string();
    let cwd = meta["cwd"].as_str().unwrap_or_default().to_string();
    let title = lines.filter_map(|l| serde_json::from_slice::<Value>(&l).ok()).find_map(|v| user_text(&v["payload"]).filter(|_| v["type"] == "response_item")).and_then(|t| t.lines().next().map(str::to_string));
    Some((id, cwd, title))
}

/// The lines of a file that say something, as read.
struct Lines {
    /// (time, payload) of each response item.
    items: Vec<(u64, Value)>,
    next_ordinal: u64,
    damaged: usize,
}

fn read_range(path: &Path, from: u64, to: u64) -> std::io::Result<Lines> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    Ok(read_lines(BufReader::new(std::io::Read::take(file, to.saturating_sub(from)))))
}

fn read_lines(reader: impl BufRead) -> Lines {
    let mut out = Lines { items: Vec::new(), next_ordinal: 0, damaged: 0 };
    let mut at_ms = 0;
    for line in reader.split(b'\n').map_while(Result::ok) {
        if line.is_empty() {
            continue;
        }
        let Ok(mut value) = serde_json::from_slice::<Value>(&line) else {
            out.damaged += 1;
            continue;
        };
        if let Some(ordinal) = value["ordinal"].as_u64() {
            out.next_ordinal = out.next_ordinal.max(ordinal + 1);
        } else {
            // A line with no number still takes one: the next is numbered past it.
            out.next_ordinal += 1;
        }
        at_ms = value["timestamp"].as_str().and_then(millis).unwrap_or(at_ms);
        if value["type"] == "response_item" {
            let mut payload = value["payload"].take();
            shrink(&mut payload);
            out.items.push((at_ms, payload));
        }
    }
    out
}

/// Tool output cut and reasoning dropped as each line is read: what keeps a long one in memory.
fn shrink(payload: &mut Value) {
    match payload["type"].as_str() {
        Some("function_call_output" | "custom_tool_call_output") => payload["output"] = Value::String(cap(&output_text(&payload["output"]))),
        Some("reasoning") => *payload = Value::Null,
        _ => {}
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
    format!("{}\n… cut on import, {} bytes in all; Codex keeps the whole of it", &text[..end], text.len())
}

/// A tool's output as text: a string, or a list of text parts.
fn output_text(output: &Value) -> String {
    match output {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join(""),
        Value::Object(_) => output["content"].as_str().map(str::to_string).unwrap_or_else(|| output.to_string()),
        _ => String::new(),
    }
}

/// The text parts of a message.
fn message_text(payload: &Value) -> String {
    payload["content"].as_array().map(|parts| parts.iter().filter(|p| matches!(p["type"].as_str(), Some("input_text" | "output_text" | "text"))).filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// What the user said in this item, if they did. What Codex wraps in tags (its environment, the
/// project's instructions) is not their words.
fn user_text(payload: &Value) -> Option<String> {
    if payload["type"] != "message" || payload["role"] != "user" {
        return None;
    }
    let text = message_text(payload);
    let text = text.trim();
    let wrapped = ["<environment_context>", "<user_instructions>", "<permissions", "<turn_aborted>", "# AGENTS.md"].iter().any(|w| text.starts_with(w));
    (!text.is_empty() && !wrapped).then(|| text.to_string())
}

/// The items as the log's events: a turn from the first thing Codex does after a user's message to
/// the next message of the user's.
fn to_entries(items: &[(u64, Value)]) -> Vec<LogEntry> {
    let mut events: Vec<(u64, Event)> = Vec::new();
    let (mut turn, mut open, mut at_ms) = (0u32, false, 0u64);
    let mut calls: Vec<String> = Vec::new();
    let mut answered: HashSet<String> = HashSet::new();
    // A call with no output in the record (an interruption) must not stay running for ever.
    let close = |events: &mut Vec<(u64, Event)>, open: &mut bool, turn: u32, at_ms: u64, calls: &mut Vec<String>, answered: &mut HashSet<String>| {
        for id in calls.drain(..).filter(|id| !answered.contains(id)) {
            events.push((at_ms, Event::ToolResult { id, output: "no result in the record".to_string(), is_error: true }));
        }
        answered.clear();
        if *open {
            *open = false;
            events.push((at_ms, Event::TurnFinished { turn, stop: StopReason::EndTurn, usage: Usage::default(), cost_micro_usd: None }));
        }
    };
    for (time, item) in items {
        at_ms = *time;
        let begin = |events: &mut Vec<(u64, Event)>, open: &mut bool, turn: &mut u32| {
            if !*open {
                *open = true;
                *turn += 1;
                events.push((at_ms, Event::TurnStarted { turn: *turn, provider: NAME.to_string(), model: String::new() }));
            }
        };
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match (item["type"].as_str().unwrap_or_default(), item["role"].as_str().unwrap_or_default()) {
            ("message", "user") => {
                if let Some(said) = user_text(item) {
                    close(&mut events, &mut open, turn, at_ms, &mut calls, &mut answered);
                    events.push((at_ms, Event::UserMessage { text: said }));
                }
            }
            ("message", "assistant") => {
                let said = message_text(item);
                if !said.is_empty() {
                    begin(&mut events, &mut open, &mut turn);
                    events.push((at_ms, Event::TextDelta { text: said }));
                }
            }
            ("function_call" | "custom_tool_call", _) => {
                begin(&mut events, &mut open, &mut turn);
                let id = text(&item["call_id"]);
                // A function's arguments are a JSON string; a custom tool's input is free text.
                let input = if item["type"] == "function_call" {
                    item["arguments"].as_str().and_then(|a| serde_json::from_str(a).ok()).unwrap_or_else(|| item["arguments"].clone())
                } else {
                    json!({"input": item["input"]})
                };
                let call = ToolCall { id: id.clone(), name: text(&item["name"]), input };
                events.push((at_ms, Event::ModelToolCall { call: call.clone() }));
                events.push((at_ms, Event::ToolRequested { call, capabilities: vec![] }));
                calls.push(id);
            }
            ("function_call_output" | "custom_tool_call_output", _) => {
                let id = text(&item["call_id"]);
                if answered.insert(id.clone()) {
                    let output = output_text(&item["output"]);
                    // How the harness writes a failed result for Codex (lines_of): read back as one.
                    let (output, is_error) = match output.strip_prefix("[error] ") {
                        Some(rest) => (rest.to_string(), true),
                        None => (output, false),
                    };
                    events.push((at_ms, Event::ToolResult { id, output, is_error }));
                }
            }
            _ => {}
        }
    }
    close(&mut events, &mut open, turn, at_ms, &mut calls, &mut answered);
    events.into_iter().enumerate().map(|(seq, (at_ms, event))| LogEntry { seq: seq as u64, at_ms, event }).collect()
}

/// A history as rollout lines numbered from `ordinal`, and the ordinal after them. Reasoning stays
/// out: it is the provider's own, made for the conversation it was made in.
fn lines_of(ordinal: u64, stamp: &str, history: &[Message]) -> (String, u64) {
    let mut out = Vec::new();
    let next = write_lines(&mut out, ordinal, stamp, history).expect("writing to memory");
    (String::from_utf8(out).expect("JSON is UTF-8"), next)
}

/// The same, written as it is made: a whole conversation is never held twice (journal#955).
fn write_lines(out: &mut dyn std::io::Write, mut ordinal: u64, stamp: &str, history: &[Message]) -> std::io::Result<u64> {
    let mut failed: Option<std::io::Error> = None;
    let mut push = |payload: Value, ordinal: &mut u64| {
        if failed.is_none() {
            let line = json!({"timestamp": stamp, "ordinal": *ordinal, "type": "response_item", "payload": payload}).to_string();
            if let Err(e) = out.write_all(line.as_bytes()).and_then(|()| out.write_all(b"\n")) {
                failed = Some(e);
            }
        }
        *ordinal += 1;
    };
    let total = history.len() as u64;
    for (n, message) in history.iter().enumerate() {
        harness_api::platform::progress::report("Writing Codex's copy", n as u64, total, harness_api::platform::progress::Unit::Messages);
        let (role, kind) = match message.role {
            Role::User => ("user", "input_text"),
            Role::Assistant => ("assistant", "output_text"),
        };
        // Text parts go together as one message, in their place among the calls.
        let mut texts: Vec<Value> = Vec::new();
        let flush = |texts: &mut Vec<Value>, push: &mut dyn FnMut(Value, &mut u64), ordinal: &mut u64| {
            if !texts.is_empty() {
                push(json!({"type": "message", "role": role, "content": std::mem::take(texts)}), ordinal);
            }
        };
        for content in &message.content {
            match content {
                Content::Text { text } => texts.push(if role == "assistant" { json!({"type": kind, "text": text, "annotations": []}) } else { json!({"type": kind, "text": text}) }),
                Content::ToolUse { id, name, input } => {
                    flush(&mut texts, &mut push, &mut ordinal);
                    push(json!({"type": "function_call", "name": name, "arguments": input.to_string(), "call_id": id}), &mut ordinal);
                }
                Content::ToolResult { id, output, is_error } => {
                    flush(&mut texts, &mut push, &mut ordinal);
                    let output = if *is_error { format!("[error] {output}") } else { output.clone() };
                    push(json!({"type": "function_call_output", "call_id": id, "output": output}), &mut ordinal);
                }
                Content::Opaque { .. } => {}
            }
        }
        flush(&mut texts, &mut push, &mut ordinal);
    }
    harness_api::platform::progress::report("Writing Codex's copy", total, total, harness_api::platform::progress::Unit::Messages);
    match failed {
        Some(e) => Err(e),
        None => Ok(ordinal),
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A version 7 uuid, the form of Codex's own ids: the time first, then random bits.
fn uuid_v7(millis: i64) -> String {
    use std::hash::{BuildHasher, Hasher};
    let random = || std::collections::hash_map::RandomState::new().build_hasher().finish();
    let (a, b) = (random(), random());
    let t = millis as u64 & 0xffff_ffff_ffff;
    format!("{:08x}-{:04x}-7{:03x}-{:04x}-{:012x}", t >> 16, t & 0xffff, a & 0xfff, 0x8000 | ((b >> 48) & 0x3fff), b & 0xffff_ffff_ffff)
}

/// A time as Codex writes one: RFC 3339 in UTC, to the millisecond.
fn rfc3339(millis: i64) -> String {
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

/// The time of an RFC 3339 stamp in UTC.
fn millis(stamp: &str) -> Option<u64> {
    let number = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    let fraction = stamp.get(20..23).and_then(|f| f.parse::<i64>().ok()).unwrap_or(0);
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468;
    u64::try_from((days * 86_400 + hour * 3600 + minute * 60 + second) * 1000 + fraction).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(ordinal: Option<u64>, kind: &str, payload: Value) -> String {
        let mut v = json!({"timestamp": "2026-11-01T10:00:00.500Z", "type": kind, "payload": payload});
        if let Some(n) = ordinal {
            v["ordinal"] = json!(n);
        }
        v.to_string()
    }

    /// The shape of a rollout Codex 0.166.0 wrote in the spike (journal#918), cut down.
    fn recorded() -> Vec<String> {
        vec![
            line(Some(0), "session_meta", json!({"id": "T1", "session_id": "T1", "cwd": "/work/p", "cli_version": "0.166.0"})),
            line(Some(1), "event_msg", json!({"type": "task_started"})),
            line(Some(2), "response_item", json!({"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "You are Codex"}]})),
            line(Some(3), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>\n<cwd>/work/p</cwd>"}]})),
            line(Some(4), "turn_context", json!({"cwd": "/work/p"})),
            line(Some(5), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Run cat note.txt"}]})),
            line(Some(6), "response_item", json!({"type": "reasoning", "summary": [], "encrypted_content": "SECRET"})),
            line(Some(7), "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "I'll read it."}], "phase": "commentary"})),
            line(Some(8), "response_item", json!({"type": "custom_tool_call", "call_id": "c1", "name": "exec", "input": "text(await tools.exec_command({cmd:\"cat note.txt\"}));"})),
            line(Some(9), "response_item", json!({"type": "custom_tool_call_output", "call_id": "c1", "output": [{"type": "input_text", "text": "Output:\n"}, {"type": "input_text", "text": "spike"}]})),
            line(Some(10), "response_item", json!({"type": "function_call", "call_id": "c2", "name": "shell", "arguments": "{\"command\":[\"ls\"]}"})),
            line(Some(11), "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "File content: spike"}]})),
            line(Some(12), "event_msg", json!({"type": "task_complete"})),
        ]
    }

    fn put(dir: &Path, id: &str, lines: &[String]) -> PathBuf {
        let folder = dir.join("sessions/2026/11/01");
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("rollout-2026-11-01T10-00-00-{id}.jsonl"));
        let mut text = lines.join("\n");
        text.push('\n');
        std::fs::write(&path, text).unwrap();
        path
    }

    fn kinds(entries: &[LogEntry]) -> Vec<&'static str> {
        entries.iter().map(|e| match &e.event {
            Event::UserMessage { .. } => "user", Event::TurnStarted { .. } => "start", Event::TextDelta { .. } => "text", Event::ModelToolCall { .. } => "model-call", Event::ToolRequested { .. } => "card",
            Event::TurnFinished { .. } => "finish", Event::ToolResult { .. } => "result", Event::ConversationImported { .. } => "imported", Event::FormSynced { .. } => "form", _ => "other",
        }).collect()
    }

    fn said(entries: &[LogEntry]) -> Vec<String> {
        entries.iter().filter_map(|e| match &e.event {
            Event::UserMessage { text } | Event::TextDelta { text } => Some(text.clone()),
            _ => None,
        }).collect()
    }

    #[test]
    fn a_recorded_rollout_reads_as_messages_and_tool_calls() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "T1", &recorded());
        let form = CodexConversations::new(dir.path());
        let c = form.read("/work/p", "T1").unwrap();
        assert_eq!(kinds(&c.entries), ["user", "start", "text", "model-call", "card", "result", "model-call", "card", "text", "result", "finish", "imported", "form"]);
        assert_eq!(said(&c.entries), ["Run cat note.txt", "I'll read it.", "File content: spike"]);
        let text = format!("{:?}", c.entries);
        assert!(!text.contains("SECRET") && !text.contains("You are Codex") && !text.contains("environment_context"), "{text}");
        assert!(c.entries.iter().any(|e| matches!(&e.event, Event::ToolResult { id, output, is_error: false } if id == "c1" && output == "Output:\nspike")));
        assert!(c.entries.iter().any(|e| matches!(&e.event, Event::ToolResult { id, is_error: true, .. } if id == "c2")), "a call with no output does not stay running");
        assert!(c.entries.iter().any(|e| matches!(&e.event, Event::ModelToolCall { call } if call.name == "shell" && call.input == json!({"command": ["ls"]}))));
        assert!(matches!(&c.entries.last().unwrap().event, Event::FormSynced { cursor, .. } if cursor.ends_with(":13")), "{:?}", c.entries.last());
        assert_eq!(c.info.title, "Run cat note.txt");
        assert_eq!(c.entries[0].at_ms, 1_793_527_200_500);
        let listed = form.list("/work/p").unwrap();
        assert_eq!((listed.len(), listed[0].id.as_str(), listed[0].title.as_str()), (1, "T1", "Run cat note.txt"));
        assert!(form.list("/elsewhere").unwrap().is_empty());
        assert!(form.read("/work/p", "../x").is_err());
        assert!(form.read("/work/p", "nope").is_err());
    }

    /// The log is the format: what came from one provider's form goes into Codex's and back whole.
    #[test]
    fn a_history_written_reads_back_and_is_valid_for_a_provider() {
        let text = |t: &str| Content::Text { text: t.to_string() };
        let history = vec![
            Message { role: Role::User, content: vec![text("first")] },
            Message { role: Role::Assistant, content: vec![text("running"), Content::Opaque { block: json!({"type": "thinking", "signature": "other's"}) }, Content::ToolUse { id: "t1".into(), name: "Bash".into(), input: json!({"command": "ls"}) }] },
            Message { role: Role::User, content: vec![Content::ToolResult { id: "t1".into(), output: "a.txt".into(), is_error: false }] },
            Message { role: Role::Assistant, content: vec![text("again"), Content::ToolUse { id: "t2".into(), name: "Bash".into(), input: json!({"command": "false"}) }] },
            Message { role: Role::User, content: vec![Content::ToolResult { id: "t2".into(), output: "exit 1".into(), is_error: true }] },
            Message { role: Role::Assistant, content: vec![text("done")] },
        ];
        let dir = tempfile::tempdir().unwrap();
        let form = CodexConversations::new(dir.path());
        let id = form.write("/work/p", "my talk", &history).unwrap();
        assert_eq!((id.len(), id.as_bytes()[14]), (36, b'7'), "a version 7 uuid: {id}");
        let path = form.file(&id).unwrap().unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("rollout-") && name.ends_with(&format!("-{id}.jsonl")), "{name}");
        let ordinals: Vec<u64> = std::fs::read_to_string(&path).unwrap().lines().map(|l| serde_json::from_str::<Value>(l).unwrap()["ordinal"].as_u64().unwrap()).collect();
        assert_eq!(ordinals, (0..ordinals.len() as u64).collect::<Vec<_>>(), "every line numbered in turn");
        let back = form.read("/work/p", &id).unwrap();
        assert!(!format!("{:?}", back.entries).contains("other's"));
        let state = SessionState::fold(back.entries.iter().map(|e| &e.event));
        let mut expected = history.clone();
        expected[1].content.remove(1);
        assert_eq!(provider_ready(&state.messages), expected);
        assert_eq!(form.list("/work/p").unwrap()[0].id, id);
        assert!(form.exists("/work/p", &id) && !form.exists("/work/p", "01a1cc4a-none"));
    }

    /// INV-20, INV-21: appended at the tip with every earlier byte kept, numbered on; what Codex
    /// added pulled once; a half-written line waits; an old position is refused.
    #[test]
    fn a_form_is_appended_to_and_pulled_from_by_position() {
        let dir = tempfile::tempdir().unwrap();
        let path = put(dir.path(), "T1", &recorded());
        let form = CodexConversations::new(dir.path());
        let cursor = form.tip("/work/p", "T1").unwrap();
        assert!(cursor.ends_with(":13"), "{cursor}");
        assert!(form.pull("/work/p", "T1", &cursor).unwrap().entries.is_empty());
        let before = std::fs::read(&path).unwrap();

        let second = form.append("/work/p", "T1", &cursor, &[Message::user("two".to_string()), Message { role: Role::Assistant, content: vec![Content::Text { text: "three".into() }] }]).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[..before.len()], &before[..], "only appended");
        assert!(second.ends_with(":15"), "{second}");
        assert!(form.pull("/work/p", "T1", &second).unwrap().entries.is_empty(), "its own lines do not come back");
        assert!(form.append("/work/p", "T1", &cursor, &[Message::user("x".to_string())]).is_err(), "not from an old position");
        assert_eq!(said(&form.read("/work/p", "T1").unwrap().entries)[3..], ["two", "three"]);

        // Codex goes on: a turn of its own, and a line it has not finished.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", line(Some(15), "event_msg", json!({"type": "task_started"}))).unwrap();
        writeln!(file, "{}", line(Some(16), "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "four"}]}))).unwrap();
        writeln!(file, "{}", line(Some(17), "response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "five"}]}))).unwrap();
        write!(file, "{{\"type\":\"response_item\",\"half").unwrap();
        let pulled = form.pull("/work/p", "T1", &second).unwrap();
        assert_eq!(said(&pulled.entries), ["four", "five"]);
        assert_eq!(kinds(&pulled.entries), ["user", "start", "text", "finish"]);
        assert!(!pulled.rewound && pulled.cursor.ends_with(":18"), "{}", pulled.cursor);
        assert!(form.pull("/work/p", "T1", &pulled.cursor).unwrap().entries.is_empty(), "read once; the half line waits");
        assert_eq!(form.tip_after("/work/p", "T1", &second).unwrap(), pulled.cursor);

        // Written over, shorter: changed outside Codex, and nothing of it comes in as new.
        std::fs::write(&path, format!("{}\n", recorded()[..6].join("\n"))).unwrap();
        assert!(form.pull("/work/p", "T1", &pulled.cursor).unwrap_err().to_string().contains("changed outside"));
    }

    #[test]
    fn a_line_with_no_number_still_moves_the_next() {
        let mut lines = recorded();
        lines.push(line(None, "response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": "x"}]})));
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), "T1", &lines);
        assert!(CodexConversations::new(dir.path()).tip("/work/p", "T1").unwrap().ends_with(":14"));
    }

    /// CON-78: a codex process that resumes the conversation holds it; nothing else does.
    #[cfg(unix)]
    #[test]
    fn a_conversation_is_held_by_a_codex_resuming_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex");
        std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let id = "01a1cc4a-held-7000-8000-000000000001";
        let form = CodexConversations::new(dir.path());
        assert_eq!(form.held_by(id, None), None);
        let mut child = std::process::Command::new(&fake).args(["exec", "--json", "resume", id, "-"]).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let held = form.held_by(id, None);
        let mine = form.held_by(id, Some(child.id()));
        let other = form.held_by("01a1cc4a-other", None);
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(held.is_some_and(|h| h.contains("Codex")));
        assert_eq!(mine, None, "its own process does not count");
        assert_eq!(other, None);
    }
}
