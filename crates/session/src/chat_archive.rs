//! A conversation dumped whole and loaded back, on this machine or another (CON-171): what the
//! command line's `multitool chat dump|load` and the Settings' Chats tab both run.
//!
//! The archive is a .zip:
//! - `manifest.json`: the format, the multitool session id, its name, its nick, the project's
//!   path where it was dumped, its adapter and the Claude Code conversation it is linked to;
//! - `multitool/<session>.jsonl`: the harness's own log of the conversation;
//! - `claude/<id>.jsonl` and `claude/<id>/…` (subagents, spilled tool results), and
//!   `claude-file-history/<id>/…` (the /rewind backups): the linked Claude Code form, whole;
//! - `journal.jsonl`: the project's journal with its mind maps and flowcharts, as
//!   `multitool journal export` writes it.
//!
//! A dump never writes over a file: its default name carries the time of the dump, and `-2`, `-3`… when
//! that is taken. A load gives the conversation new ids, always (CON-175), moves `cwd` fields
//! under the dumped project to the folder it is loaded in, and brings the journal in once: new ids
//! here, its maps' references moved with them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use harness_api::Event;
use serde::{Deserialize, Serialize};

/// The folder of a project where its dumps go, by default (journal#1288).
pub const DUMPS_DIR: &str = "multitool_dumps";

/// This archive layout's version.
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub format: u32,
    /// The multitool session id.
    pub session: String,
    /// The name it is known by, as /resume and the start page show it.
    pub name: String,
    #[serde(default)]
    pub nick: Option<String>,
    /// The project's folder where it was dumped.
    pub project: String,
    pub adapter: String,
    /// The Claude Code conversation it is linked to, if any.
    #[serde(default)]
    pub claude_id: Option<String>,
    pub dumped_at_ms: u64,
}

/// What a dump wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dumped {
    pub path: PathBuf,
    pub bytes: u64,
    pub files: usize,
    pub name: String,
    /// Whether the project's journal went in.
    pub journal: bool,
}

/// What a load did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub session: String,
    pub name: String,
    pub claude_id: Option<String>,
    pub moved_from: String,
    pub journal_written: usize,
    pub journal_merged: usize,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Another name to list it under.
    pub name: Option<String>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Claude Code's configuration folder as the user's own Claude Code finds it.
fn claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".claude"))
}

/// The folder Claude Code keeps a project's conversations in, under its configuration folder.
fn claude_folder(claude: &Path, root: &str) -> PathBuf {
    let slug: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    claude.join("projects").join(slug)
}

fn log_path(data_dir: &str, root: &str, session: &str) -> PathBuf {
    PathBuf::from(crate::sessions_dir(data_dir, root)).join(format!("{session}.jsonl"))
}

/// A session of this project by its id, or by the name the start page shows when one has it.
pub fn resolve(data_dir: &str, root: &str, wanted: &str) -> Result<String, String> {
    let recent = crate::recent(data_dir, root);
    if recent.iter().any(|s| s.id == wanted) {
        return Ok(wanted.to_string());
    }
    let named: Vec<&crate::SessionInfo> = recent.iter().filter(|s| s.title == wanted || s.id.starts_with(wanted)).collect();
    match named.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("no conversation of this project is called or numbered {wanted:?}")),
        many => Err(format!("{} conversations match {wanted:?}: {}", many.len(), many.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "))),
    }
}

/// What the log says of a session: its linked Claude Code id and its nick.
fn of_log(log: &str) -> (Option<String>, Option<String>) {
    let (mut claude, mut nick) = (None, None);
    for line in log.lines() {
        let Ok(entry) = harness_api::event::from_line(line) else { continue };
        match entry.event {
            Event::ConversationImported { source, id, .. } | Event::FormSynced { source, id, .. } if source == "claude-code" => claude = Some(id),
            Event::AgentName { name, .. } => nick = Some(name),
            _ => {}
        }
    }
    (claude, nick)
}

/// A name fit for a file's.
fn file_name(name: &str) -> String {
    let cleaned: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let trimmed: String = cleaned.trim_matches('-').chars().take(60).collect();
    if trimmed.is_empty() { "conversation".to_string() } else { trimmed }
}

/// How far a dump or a load is, in bytes (CON-172): told to `report` at most every 100 ms, and
/// once more at the end.
struct Counter<'a> {
    done: std::cell::Cell<u64>,
    total: u64,
    last: std::cell::Cell<Option<std::time::Instant>>,
    report: &'a dyn Fn(u64, u64),
}

impl<'a> Counter<'a> {
    fn new(total: u64, report: &'a dyn Fn(u64, u64)) -> Self {
        report(0, total);
        Counter { done: Default::default(), total, last: std::cell::Cell::new(Some(std::time::Instant::now())), report }
    }

    fn add(&self, n: u64) {
        self.done.set(self.done.get() + n);
        if self.last.get().is_none_or(|at| at.elapsed() >= std::time::Duration::from_millis(100)) {
            self.last.set(Some(std::time::Instant::now()));
            (self.report)(self.done.get().min(self.total), self.total);
        }
    }

    fn finish(&self) {
        (self.report)(self.total, self.total);
    }
}

/// A reader that counts what goes through it, so the count moves inside a big file too.
struct Counted<'a, 'b, R> {
    inner: R,
    counter: &'a Counter<'b>,
}

impl<R: Read> Read for Counted<'_, '_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.counter.add(n as u64);
        Ok(n)
    }
}

fn add_file(zip: &mut zip::ZipWriter<std::fs::File>, name: &str, path: &Path, counter: &Counter) -> Result<(), String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?.len();
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(size >= 0xFFFF_FFFF);
    zip.start_file(name, options).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::io::copy(&mut Counted { inner: file, counter }, zip).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

fn add_bytes(zip: &mut zip::ZipWriter<std::fs::File>, name: &str, bytes: &[u8]) -> Result<(), String> {
    zip.start_file(name, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).map_err(|e| e.to_string())?;
    zip.write_all(bytes).map_err(|e| e.to_string())
}

/// Every file under `dir`, with its path relative to it, '/'-separated.
fn files_under(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&at) else { continue };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                out.push((rel.to_string_lossy().replace('\\', "/"), path));
            }
        }
    }
    out.sort();
    out
}

/// Packs `session` of the project at `root` into a .zip: `out`, else `<name>-<id>-<yyyy-mm-dd-hh-mm-ss>.zip`
/// in `root`'s multitool_dumps/ (journal#1288), `-2`, `-3`… after it when that is taken (journal#1278).
pub fn dump(data_dir: &str, root: &str, session: &str, out: Option<&Path>) -> Result<Dumped, String> {
    dump_with(data_dir, root, session, out, claude_dir(), &|_, _| {})
}

/// As [`dump`], telling `progress` the bytes packed so far and in all (CON-172).
pub fn dump_progress(data_dir: &str, root: &str, session: &str, out: Option<&Path>, progress: &dyn Fn(u64, u64)) -> Result<Dumped, String> {
    dump_with(data_dir, root, session, out, claude_dir(), progress)
}

/// The first of `<stem>.zip`, `<stem>-2.zip`, `<stem>-3.zip`… in `root` that is not there (journal#1278).
fn free_path(root: &Path, stem: &str) -> PathBuf {
    (1..).map(|n| root.join(if n == 1 { format!("{stem}.zip") } else { format!("{stem}-{n}.zip") })).find(|p| !p.exists() && !p.with_extension("zip.partial").exists()).expect("some name is free")
}

fn dump_with(data_dir: &str, root: &str, session: &str, out: Option<&Path>, claude: Option<PathBuf>, progress: &dyn Fn(u64, u64)) -> Result<Dumped, String> {
    let log = log_path(data_dir, root, session);
    let text = std::fs::read_to_string(&log).map_err(|_| format!("no conversation {session} in this project"))?;
    let (claude_id, nick) = of_log(&text);
    let name = crate::recent(data_dir, root).into_iter().find(|s| s.id == session).map_or_else(|| session.to_string(), |s| s.title);
    let short: String = claude_id.as_deref().unwrap_or(session).chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    let path = match out {
        Some(out) if out.exists() => return Err(format!("{} is there already, and is never written over: give another name with --out", out.display())),
        Some(out) => out.to_path_buf(),
        None => {
            let folder = Path::new(root).join(DUMPS_DIR);
            std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
            free_path(&folder, &format!("{}-{short}-{}", file_name(&name), chrono::Local::now().format("%Y-%m-%d-%H-%M-%S")))
        }
    };
    let adapter = std::fs::read_to_string(Path::new(data_dir).join("daemon/conversations.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v[session]["provider"].as_str().map(str::to_string))
        .unwrap_or_else(|| if claude_id.is_some() { "claude-code".to_string() } else { "native".to_string() });
    let manifest = Manifest { format: FORMAT, session: session.to_string(), name: name.clone(), nick, project: root.to_string(), adapter, claude_id: claude_id.clone(), dumped_at_ms: now_ms() };
    // What goes in, known before the first byte is packed, so the progress has its total.
    let mut sources = vec![(format!("multitool/{session}.jsonl"), log.clone())];
    if let (Some(id), Some(claude)) = (&claude_id, &claude) {
        let folder = claude_folder(claude, root);
        let transcript = folder.join(format!("{id}.jsonl"));
        if transcript.is_file() {
            sources.push((format!("claude/{id}.jsonl"), transcript));
        }
        sources.extend(files_under(&folder.join(id)).into_iter().map(|(rel, file)| (format!("claude/{id}/{rel}"), file)));
        sources.extend(files_under(&claude.join("file-history").join(id)).into_iter().map(|(rel, file)| (format!("claude-file-history/{id}/{rel}"), file)));
    }
    // The project's journal, with its mind maps and flowcharts (CON-171).
    let journal = crate::journal_export(data_dir, root).ok();
    let total = sources.iter().map(|(_, p)| std::fs::metadata(p).map_or(0, |m| m.len())).sum::<u64>() + journal.as_ref().map_or(0, |j| j.len() as u64);
    let counter = Counter::new(total, progress);
    let partial = path.with_extension("zip.partial");
    let file = std::fs::File::create(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let mut files = 0;
    let written = (|| -> Result<(), String> {
        add_bytes(&mut zip, "manifest.json", serde_json::to_string_pretty(&manifest).map_err(|e| e.to_string())?.as_bytes())?;
        files += 1;
        for (name, source) in &sources {
            add_file(&mut zip, name, source, &counter)?;
            files += 1;
        }
        if let Some(journal) = &journal {
            add_bytes(&mut zip, "journal.jsonl", journal.as_bytes())?;
            counter.add(journal.len() as u64);
            files += 1;
        }
        Ok(())
    })();
    let finished = zip.finish().map_err(|e| e.to_string());
    if let Some(e) = written.err().or(finished.err()) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    counter.finish();
    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
    Ok(Dumped { path, bytes, files, name, journal: journal.is_some() })
}

fn new_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// One line of a transcript or a log, its project paths moved and its ids renamed. A line that is
/// not whole JSON (the unfinished last one) is left out.
fn moved_line(line: &str, from: &str, to: &str, renamed: &[(String, String)]) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(line).ok()?;
    if let Some(cwd) = value.get("cwd").and_then(|c| c.as_str())
        && (cwd == from || cwd.starts_with(&format!("{from}/")))
    {
        value["cwd"] = serde_json::Value::String(format!("{to}{}", &cwd[from.len()..]));
    }
    let mut text = value.to_string();
    for (old, new) in renamed {
        text = text.replace(old.as_str(), new);
    }
    Some(text)
}

fn moved_text(text: &str, from: &str, to: &str, renamed: &[(String, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        if let Some(line) = moved_line(line, from, to, renamed) {
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn read_entry(archive: &mut zip::ZipArchive<std::fs::File>, name: &str, counter: &Counter) -> Result<Option<Vec<u8>>, String> {
    let Ok(file) = archive.by_name(name) else { return Ok(None) };
    let mut bytes = Vec::new();
    Counted { inner: file, counter }.read_to_end(&mut bytes).map_err(|e| format!("{name}: {e}"))?;
    Ok(Some(bytes))
}

fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Brings a dumped conversation into the project at `root`.
pub fn load(data_dir: &str, root: &str, archive_path: &Path, options: &LoadOptions) -> Result<Loaded, String> {
    load_with(data_dir, root, archive_path, options, claude_dir(), &|_, _| {})
}

/// As [`load`], telling `progress` the bytes unpacked so far and in all (CON-172).
pub fn load_progress(data_dir: &str, root: &str, archive_path: &Path, options: &LoadOptions, progress: &dyn Fn(u64, u64)) -> Result<Loaded, String> {
    load_with(data_dir, root, archive_path, options, claude_dir(), progress)
}

fn load_with(data_dir: &str, root: &str, archive_path: &Path, options: &LoadOptions, claude: Option<PathBuf>, progress: &dyn Fn(u64, u64)) -> Result<Loaded, String> {
    let file = std::fs::File::open(archive_path).map_err(|e| format!("{}: {e}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{} is not a chat archive: {e}", archive_path.display()))?;
    let total = (0..archive.len()).filter_map(|i| archive.by_index_raw(i).ok().map(|f| f.size())).sum();
    let counter = Counter::new(total, progress);
    let manifest: Manifest = serde_json::from_slice(&read_entry(&mut archive, "manifest.json", &counter)?.ok_or("no manifest.json: not a chat archive")?).map_err(|e| format!("manifest.json: {e}"))?;
    if manifest.format > FORMAT {
        return Err(format!("the archive is of a newer format ({}) than this multitool reads ({FORMAT})", manifest.format));
    }
    // New ids, always (CON-175): the session's, and Claude Code's, renamed wherever they are written.
    let session = format!("loaded-{}-{}", &new_uuid()[..8], now_ms());
    let claude_id = manifest.claude_id.as_ref().map(|_| new_uuid());
    let mut renamed: Vec<(String, String)> = Vec::new();
    if let (Some(old), Some(new)) = (&manifest.claude_id, &claude_id)
        && old != new
    {
        renamed.push((old.clone(), new.clone()));
    }
    let name = options.name.clone().unwrap_or_else(|| manifest.name.clone());
    // The harness's own log.
    let log = read_entry(&mut archive, &format!("multitool/{}.jsonl", manifest.session), &counter)?.ok_or("the archive has no multitool log")?;
    let mut log = moved_text(&String::from_utf8_lossy(&log), &manifest.project, root, &renamed);
    if let Some(wanted) = &options.name {
        log = log
            .lines()
            .map(|line| match harness_api::event::from_line(line) {
                Ok(mut entry) if matches!(entry.event, Event::ConversationImported { .. }) => {
                    if let Event::ConversationImported { title, .. } = &mut entry.event {
                        title.clone_from(wanted);
                    }
                    harness_api::event::to_line(&entry)
                }
                _ => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
    }
    write(&log_path(data_dir, root, &session), log.as_bytes())?;
    // Claude Code's form, at this folder's place.
    if let (Some(old), Some(new), Some(claude)) = (&manifest.claude_id, &claude_id, &claude) {
        let folder = claude_folder(claude, root);
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        for entry in names {
            let (target, text) = if let Some(rest) = entry.strip_prefix(&format!("claude/{old}")) {
                (folder.join(format!("{new}{rest}")), entry.ends_with(".jsonl"))
            } else if let Some(rest) = entry.strip_prefix(&format!("claude-file-history/{old}/")) {
                (claude.join("file-history").join(new).join(rest), false)
            } else {
                continue;
            };
            let bytes = read_entry(&mut archive, &entry, &counter)?.unwrap_or_default();
            let bytes = if text { moved_text(&String::from_utf8_lossy(&bytes), &manifest.project, root, &renamed).into_bytes() } else { bytes };
            write(&target, &bytes)?;
        }
        if let Some(wanted) = &options.name {
            let transcript = folder.join(format!("{new}.jsonl"));
            let line = serde_json::json!({"type": "custom-title", "customTitle": wanted, "sessionId": new}).to_string() + "\n";
            std::fs::OpenOptions::new().append(true).open(&transcript).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| format!("{}: {e}", transcript.display()))?;
        }
    }
    // The journal: once, with new ids here.
    let (mut journal_written, mut journal_merged) = (0, 0);
    if let Some(journal) = read_entry(&mut archive, "journal.jsonl", &counter)? {
        (journal_written, journal_merged) = merge_journal(data_dir, root, &String::from_utf8_lossy(&journal))?;
    }
    counter.finish();
    Ok(Loaded { session, name, claude_id, moved_from: manifest.project, journal_written, journal_merged })
}

/// A journal stream into this project's journal: its entries once (what is here already is merged,
/// not written twice), under new ids, and its maps with their references moved to them.
pub fn merge_journal(data_dir: &str, root: &str, stream: &str) -> Result<(usize, usize), String> {
    use harness_memory_journal::journal;
    let scratch = std::env::temp_dir().join(format!("mt-chat-load-{}-{}", std::process::id(), now_ms()));
    let key = Path::new("chat-archive");
    let result = (|| -> Result<(usize, usize), String> {
        harness_memory_journal::transfer::import(&scratch, key, stream, false).map_err(|e| format!("the archive's journal: {e:#}"))?;
        let src = journal::open(&scratch, key, false).map_err(|e| e.to_string())?.ok_or("the archive's journal did not open")?;
        let here = harness_memory_journal::project::key_for(crate::git_origin(root).as_deref(), Path::new(root));
        let mut dst = journal::open(&Path::new(data_dir).join("journals"), &here, true).map_err(|e| e.to_string())?.ok_or("this project's journal did not open")?;
        journal::merge_into(&mut dst, &src).map_err(|e| format!("{e:#}"))
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// The conversations of a project for the Chats tab: id, name, adapter, the size a dump takes in
/// (the log with the linked Claude Code form and its sidecars, journal#1273), newest first.
pub fn listed(data_dir: &str, root: &str) -> Vec<(crate::SessionInfo, String, u64)> {
    listed_with(data_dir, root, claude_dir())
}

fn dir_size(dir: &Path) -> u64 {
    files_under(dir).iter().map(|(_, f)| std::fs::metadata(f).map_or(0, |m| m.len())).sum()
}

fn listed_with(data_dir: &str, root: &str, claude: Option<PathBuf>) -> Vec<(crate::SessionInfo, String, u64)> {
    let adapters: HashMap<String, String> = std::fs::read_to_string(Path::new(data_dir).join("daemon/conversations.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<HashMap<String, serde_json::Value>>(&t).ok())
        .map(|m| m.into_iter().filter_map(|(k, v)| Some((k, v["provider"].as_str()?.to_string()))).collect())
        .unwrap_or_default();
    crate::recent(data_dir, root)
        .into_iter()
        .map(|s| {
            let adapter = adapters.get(&s.id).cloned().unwrap_or_else(|| if s.imported.is_some() { "claude-code".to_string() } else { "native".to_string() });
            let log = log_path(data_dir, root, &s.id);
            let mut bytes = std::fs::metadata(&log).map_or(0, |m| m.len());
            // The Claude Code form: the import's id when there is one, else the log's own link.
            let claude_id = s.imported.clone().or_else(|| std::fs::read_to_string(&log).ok().and_then(|t| of_log(&t).0));
            if let (Some(id), Some(claude)) = (claude_id, &claude) {
                let folder = claude_folder(claude, root);
                bytes += std::fs::metadata(folder.join(format!("{id}.jsonl"))).map_or(0, |m| m.len());
                bytes += dir_size(&folder.join(&id)) + dir_size(&claude.join("file-history").join(&id));
            }
            (s, adapter, bytes)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: serde_json::Value) -> String {
        format!("{v}\n")
    }

    #[test]
    fn a_line_moves_its_project_paths_and_ids_and_an_unfinished_one_is_left_out() {
        let renamed = vec![("aaaa-1111".to_string(), "bbbb-2222".to_string())];
        let text = line(serde_json::json!({"cwd": "/old/proj/sub", "sessionId": "aaaa-1111", "message": "in /old/proj"})) + &line(serde_json::json!({"cwd": "/other"})) + "{\"half";
        let moved = moved_text(&text, "/old/proj", "/new/place", &renamed);
        let lines: Vec<serde_json::Value> = moved.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2, "the unfinished line is out");
        assert_eq!(lines[0]["cwd"], "/new/place/sub");
        assert_eq!(lines[0]["sessionId"], "bbbb-2222");
        assert_eq!(lines[0]["message"], "in /old/proj", "message text is history: not moved");
        assert_eq!(lines[1]["cwd"], "/other", "outside the project: as it was");
    }

    /// CON-171: a conversation with its Claude Code form and a journal with a map goes out and
    /// comes back in another machine's place: paths moved, the journal once. CON-175: every load
    /// has new ids, and a second load of the same archive is another conversation.
    #[test]
    fn a_conversation_goes_out_whole_and_comes_back_once() {
        use harness_memory_journal::journal;
        let base = std::env::temp_dir().join(format!("mt-chat-archive-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (data_a, claude_a, root_a) = (base.join("a/data"), base.join("a/claude"), base.join("a/proj"));
        let (data_b, claude_b, root_b) = (base.join("b/data"), base.join("b/claude"), base.join("b/elsewhere"));
        for d in [&data_a, &claude_a, &root_a, &data_b, &claude_b, &root_b] {
            std::fs::create_dir_all(d).unwrap();
        }
        let (data_a, root_a_s) = (data_a.to_string_lossy().into_owned(), root_a.to_string_lossy().into_owned());
        let (data_b, root_b_s) = (data_b.to_string_lossy().into_owned(), root_b.to_string_lossy().into_owned());
        let cid = "11111111-2222-3333-4444-555555555555";
        // A's conversation: the harness's log, Claude Code's form with its sidecars, and a journal.
        let log: Vec<String> = [
            Event::UserMessage { text: "hello".into() },
            Event::ConversationImported { source: "claude-code".into(), id: cid.into(), title: "orchestrator-9".into() },
            Event::AgentName { name: "nick-a".into(), by: "agent".into() },
        ]
        .into_iter()
        .enumerate()
        .map(|(n, event)| harness_api::event::to_line(&harness_api::LogEntry { seq: n as u64, at_ms: 1 + n as u64, event }))
        .collect();
        write(&log_path(&data_a, &root_a_s, "imported-11111111-1"), (log.join("\n") + "\n").as_bytes()).unwrap();
        let folder = claude_folder(&claude_a, &root_a_s);
        write(&folder.join(format!("{cid}.jsonl")), (line(serde_json::json!({"type": "user", "cwd": root_a_s, "sessionId": cid})) + &line(serde_json::json!({"type": "ai-title", "aiTitle": "orchestrator-9"}))).as_bytes()).unwrap();
        write(&folder.join(cid).join("subagents/agent-1.jsonl"), line(serde_json::json!({"cwd": format!("{root_a_s}/sub"), "sessionId": cid})).as_bytes()).unwrap();
        write(&folder.join(cid).join("tool-results/out.txt"), b"spilled").unwrap();
        write(&claude_a.join("file-history").join(cid).join("backup@v1"), b"before").unwrap();
        let key = harness_memory_journal::project::key_for(None, Path::new(&root_a_s));
        let mut conn = journal::open(&Path::new(&data_a).join("journals"), &key, true).unwrap().unwrap();
        let (_, e1) = journal::add(&mut conn, &journal::NewEntry { body: "the first finding of A".into(), category: "finding".into(), tags: vec!["t".into()], ..Default::default() }, 100).unwrap();
        let spec = harness_memory_journal::mindmap::NewMap { title: "plan", layout: "mindmap", direction: "right", shared: true };
        let owner = harness_memory_journal::mindmap::Owner { nick: "nick-a".into(), host: None };
        let (m, root_node) = harness_memory_journal::mindmap::create_map(&mut conn, &spec, &owner, 101).unwrap();
        harness_memory_journal::mindmap::add(&mut conn, m, &harness_memory_journal::mindmap::NewNode { kind: "fact", text: "rests on e1", parent: Some(root_node), refs: &[e1], ..Default::default() }, 102).unwrap();
        drop(conn);

        // journal#1273: the listed size is what a dump takes in, the Claude Code form with it.
        let log_bytes = std::fs::metadata(log_path(&data_a, &root_a_s, "imported-11111111-1")).unwrap().len();
        let form_bytes = std::fs::metadata(folder.join(format!("{cid}.jsonl"))).unwrap().len() + dir_size(&folder.join(cid)) + dir_size(&claude_a.join("file-history").join(cid));
        let listed = listed_with(&data_a, &root_a_s, Some(claude_a.clone()));
        assert_eq!(listed[0].2, log_bytes + form_bytes, "the log and the whole Claude Code form");
        assert!(form_bytes > 0);
        // CON-172: the progress counts up to its total, which is every byte packed.
        let seen = std::cell::RefCell::new(Vec::<(u64, u64)>::new());
        let dumped = dump_with(&data_a, &root_a_s, "imported-11111111-1", None, Some(claude_a.clone()), &|done, total| seen.borrow_mut().push((done, total))).unwrap();
        let name = dumped.path.file_name().unwrap().to_string_lossy().into_owned();
        let stamp = name.strip_prefix("orchestrator-9-11111111-").and_then(|r| r.strip_suffix(".zip")).unwrap_or_default();
        assert!(dumped.path.parent() == Some(root_a.join("multitool_dumps").as_path()) && chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d-%H-%M-%S").is_ok(), "into the project's multitool_dumps/ (journal#1288), named after it with the time of the dump (journal#1284): {name}");
        assert!(dumped.journal && dumped.files == 7, "{dumped:?}");
        let seen = seen.into_inner();
        let journal_bytes = crate::journal_export(&data_a, &root_a_s).unwrap().len() as u64;
        assert_eq!(seen.last().copied(), Some((log_bytes + form_bytes + journal_bytes, log_bytes + form_bytes + journal_bytes)), "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "never back: {seen:?}");
        // journal#1278: a taken name is never written over: the next free one is taken.
        let first = std::fs::read(&dumped.path).unwrap();
        let again = dump_with(&data_a, &root_a_s, "imported-11111111-1", None, Some(claude_a.clone()), &|_, _| {}).unwrap();
        assert!(again.path != dumped.path && again.path.exists(), "a second dump is a file of its own");
        std::fs::remove_file(&again.path).unwrap();
        assert_eq!(std::fs::read(&dumped.path).unwrap(), first, "the first archive as it was");
        assert!(dump_with(&data_a, &root_a_s, "imported-11111111-1", Some(&dumped.path), Some(claude_a.clone()), &|_, _| {}).unwrap_err().contains("never written over"), "a named --out is still refused");

        // B: a filler entry first, so the journal's ids there differ from A's.
        let key_b = harness_memory_journal::project::key_for(None, Path::new(&root_b_s));
        let mut conn = journal::open(&Path::new(&data_b).join("journals"), &key_b, true).unwrap().unwrap();
        journal::add(&mut conn, &journal::NewEntry { body: "B's own older entry".into(), category: "note".into(), tags: vec!["t".into()], ..Default::default() }, 50).unwrap();
        drop(conn);
        let seen = std::cell::RefCell::new(Vec::<(u64, u64)>::new());
        let loaded = load_with(&data_b, &root_b_s, &dumped.path, &LoadOptions::default(), Some(claude_b.clone()), &|done, total| seen.borrow_mut().push((done, total))).unwrap();
        let unpacked: u64 = {
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&dumped.path).unwrap()).unwrap();
            (0..zip.len()).map(|i| zip.by_index(i).unwrap().size()).sum()
        };
        let seen = seen.into_inner();
        assert_eq!(seen.last().copied(), Some((unpacked, unpacked)), "a load counts every byte it unpacks: {seen:?}");
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0));
        assert!(loaded.session.starts_with("loaded-") && loaded.journal_written == 1, "{loaded:?}");
        let new_cid = loaded.claude_id.clone().unwrap();
        assert!(new_cid != cid && new_cid.len() == cid.len(), "a new Claude Code id: {new_cid}");
        // The harness's copy opens there with its name and nick.
        let listed = crate::recent(&data_b, &root_b_s);
        assert_eq!(listed[0].title, "orchestrator-9");
        // Claude Code's form at B's place, paths moved, sidecars there.
        let folder_b = claude_folder(&claude_b, &root_b_s);
        let transcript = std::fs::read_to_string(folder_b.join(format!("{new_cid}.jsonl"))).unwrap();
        assert!(transcript.contains(&format!("\"cwd\":\"{root_b_s}\"")) && !transcript.contains(&root_a_s) && !transcript.contains(cid), "{transcript}");
        let subagent = std::fs::read_to_string(folder_b.join(&new_cid).join("subagents/agent-1.jsonl")).unwrap();
        assert!(subagent.contains(&format!("{root_b_s}/sub")) && subagent.contains(&new_cid));
        assert_eq!(std::fs::read(folder_b.join(&new_cid).join("tool-results/out.txt")).unwrap(), b"spilled");
        assert_eq!(std::fs::read(claude_b.join("file-history").join(&new_cid).join("backup@v1")).unwrap(), b"before");
        // The journal: A's entry under a new id, its map pointing at it.
        let conn = journal::open(&Path::new(&data_b).join("journals"), &key_b, false).unwrap().unwrap();
        let (total, page) = journal::list(&conn, &journal::Filter::default(), 0, 10).unwrap();
        assert_eq!(total, 2);
        let moved = page.iter().find(|e| e.body == "the first finding of A").unwrap().id;
        let maps = harness_memory_journal::mindmap::export_all(&conn).unwrap();
        assert_eq!(maps.len(), 1);
        assert!(maps[0].nodes.iter().any(|n| n.refs == vec![moved]), "the map points at the entry's new id");
        drop(conn);

        // Again: never refused, another conversation with ids of its own; the journal not written twice.
        let copy = load_with(&data_b, &root_b_s, &dumped.path, &LoadOptions { name: Some("the copy".into()) }, Some(claude_b.clone()), &|_, _| {}).unwrap();
        assert!(copy.session != loaded.session && copy.claude_id.as_deref() != Some(cid) && copy.claude_id != loaded.claude_id);
        assert_eq!((copy.journal_written, copy.journal_merged), (0, 1));
        let copied = std::fs::read_to_string(folder_b.join(format!("{}.jsonl", copy.claude_id.unwrap()))).unwrap();
        assert!(!copied.contains(cid) && copied.contains("\"customTitle\":\"the copy\""));
        assert!(crate::recent(&data_b, &root_b_s).iter().any(|s| s.title == "the copy"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// journal#1278: a name that is taken is never written over: `-2`, `-3`… after it.
    #[test]
    fn a_taken_name_gets_the_next_number() {
        let root = std::env::temp_dir().join(format!("mt-chat-free-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(free_path(&root, "a-1"), root.join("a-1.zip"));
        std::fs::write(root.join("a-1.zip"), b"old").unwrap();
        assert_eq!(free_path(&root, "a-1"), root.join("a-1-2.zip"));
        std::fs::write(root.join("a-1-2.zip"), b"").unwrap();
        std::fs::write(root.join("a-1-3.zip.partial"), b"").unwrap();
        assert_eq!(free_path(&root, "a-1"), root.join("a-1-4.zip"), "one being written counts as taken");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_name_is_made_of_the_name() {
        assert_eq!(file_name("orchestrator-3"), "orchestrator-3");
        assert_eq!(file_name("Go-воркеры даунстрима / fbscout"), "Go-воркеры-даунстрима---fbscout");
        assert_eq!(file_name("///"), "conversation");
    }
}
