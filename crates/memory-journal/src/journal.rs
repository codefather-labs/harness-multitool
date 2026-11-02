//! The project journal: what agents decided and found, kept per project so the
//! next session can look it up by category and tag instead of starting cold.
//!
//! ## Ownership
//!
//! The daemon owns every journal. `multitool journal …` never opens these
//! files itself: it asks the daemon, which works out which project the caller
//! belongs to and is the only writer. That is what makes "the journals of every
//! project" a single place something can later orchestrate from, and it is why
//! nothing is written into the project directory.
//!
//! One SQLite file per project under `the data directory's `journals/``, named
//! `<folder name>-<12 hex of sha256(project path)>.db`: readable in a listing,
//! unique per path. The project path itself is recorded inside the file
//! (`journal_meta.project_path`), so the daemon can enumerate projects without
//! a separate index that could drift from the files.
//!
//! ## What a project is
//!
//! The directory the session was started in — the `cwd` the supervisor
//! registers with the daemon. A command run from `src/` inside that session
//! still lands in the same journal. Resolution lives in the daemon
//! (`daemon::journal_tools`); this module only takes a path.
//!
//! ## Search
//!
//! BM25 over `entries_fts` plus, when the e5 model is on disk, K-NN over
//! `entries_vec`, fused by RRF (k=60, the usual constant).
//! Metadata filters are SQL on the lexical leg; the vec0 K-NN cannot take a
//! WHERE clause, so the dense leg over-fetches and filters by id.
//!
//! SQL discipline as in `store.rs`: every value is a bound parameter. The only
//! formatted SQL is the count of `?` placeholders for an IN list.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Salience → retention. `high` is kept forever.
pub const MEDIUM_TTL_SECS: i64 = 365 * 86_400;
pub const LOW_TTL_SECS: i64 = 90 * 86_400;
/// The same body from the same project within this window is not stored twice.
pub const DEDUP_WINDOW_SECS: i64 = 30 * 86_400;
/// Longest accepted entry. A journal entry is a note, not a document.
pub const MAX_BODY_CHARS: usize = 20_000;
pub const MAX_TOP_K: usize = 50;
pub const MAX_PAGE_SIZE: usize = 100;
const RRF_K: f64 = 60.0;
const SNIPPET_CHARS: usize = 200;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS journal_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS entries (
    id              INTEGER PRIMARY KEY,
    created_at      INTEGER NOT NULL,
    category        TEXT NOT NULL,
    body            TEXT NOT NULL,
    sha256          TEXT NOT NULL,
    salience        TEXT NOT NULL DEFAULT 'medium',
    agent           TEXT,
    agent_id        TEXT,
    session_id      TEXT,
    feature         TEXT,
    source_artifact TEXT
);
CREATE INDEX IF NOT EXISTS entries_created  ON entries(created_at);
CREATE INDEX IF NOT EXISTS entries_category ON entries(category);
CREATE INDEX IF NOT EXISTS entries_sha      ON entries(sha256);
CREATE TABLE IF NOT EXISTS entry_tags (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    tag      TEXT NOT NULL,
    UNIQUE(entry_id, tag)
);
CREATE INDEX IF NOT EXISTS entry_tags_tag ON entry_tags(tag);
CREATE VIRTUAL TABLE IF NOT EXISTS entries_fts USING fts5(
    body, content='entries', content_rowid='id'
);
CREATE TRIGGER IF NOT EXISTS entries_ai AFTER INSERT ON entries BEGIN
    INSERT INTO entries_fts(rowid, body) VALUES (new.id, new.body);
END;
CREATE TRIGGER IF NOT EXISTS entries_ad AFTER DELETE ON entries BEGIN
    INSERT INTO entries_fts(entries_fts, rowid, body) VALUES ('delete', old.id, old.body);
    DELETE FROM entries_vec WHERE rowid = old.id;
END;
CREATE VIRTUAL TABLE IF NOT EXISTS entries_vec USING vec0(embedding float[384]);
CREATE TABLE IF NOT EXISTS entry_flags (
    entry_id   INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    flag       TEXT NOT NULL,
    set_at     INTEGER NOT NULL,
    closed_at  INTEGER,
    closed_by  TEXT,
    resolution TEXT,
    UNIQUE(entry_id, flag)
);
CREATE INDEX IF NOT EXISTS entry_flags_open ON entry_flags(flag, closed_at);
CREATE TABLE IF NOT EXISTS entry_links (
    from_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    to_id   INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    kind    TEXT NOT NULL,
    UNIQUE(from_id, to_id, kind)
);
CREATE INDEX IF NOT EXISTS entry_links_to ON entry_links(to_id);
CREATE TABLE IF NOT EXISTS entry_reminders (
    entry_id   INTEGER PRIMARY KEY REFERENCES entries(id) ON DELETE CASCADE,
    every_secs INTEGER NOT NULL,
    next_at    INTEGER NOT NULL,
    nick       TEXT NOT NULL,
    host       TEXT
);
CREATE TABLE IF NOT EXISTS entry_proofs (
    entry_id INTEGER NOT NULL REFERENCES entries(id) ON DELETE CASCADE,
    proof    TEXT NOT NULL,
    UNIQUE(entry_id, proof)
);
";

/// How one entry refers to another. Read from the text (`journal#N`, see
/// [`parse_refs`]) or set with `journal link`.
/// The link of an entry to one that has to be closed first. Set by `block`, which refuses what
/// could never close, so it is not among the kinds anyone can link by.
const BLOCKED_BY: &str = "blocked_by";
/// How the resolution of dropped work begins.
const ABANDONED: &str = "abandoned:";
/// An open work item on the entry named by `{}`, as SQL.
const OPEN_ITEM_SQL: &str = "SELECT 1 FROM entry_flags f WHERE f.entry_id = {} AND f.closed_at IS NULL AND f.flag NOT IN (SELECT value FROM json_each(?))";
pub const LINK_KINDS: [&str; 5] = ["cites", "replaces", "fixes", "implements", "verifies"];

/// The `journal#N` references in an entry, with what the (up to three) words
/// right before them say: "Replaces journal#42" / "supersedes" / "corrects" /
/// "reverses" -> `replaces`, "fixes journal#26" -> `fixes`, anything else ->
/// `cites`. A run
/// such as `journal#40/#41` or `journal#40, #41` names every id in it. A link
/// only records the reference: it never flags the target (an agent marks an
/// entry deprecated, deliberately — journal#41).
pub fn parse_refs(body: &str) -> Vec<(i64, &'static str)> {
    const MARK: &str = "journal#";
    let lower = body.to_lowercase();
    let mut out: Vec<(i64, &'static str)> = Vec::new();
    let mut from = 0;
    while let Some(at) = lower[from..].find(MARK).map(|i| from + i) {
        // Only the words right before the reference decide its kind: a
        // "replaces" elsewhere in the sentence ("a 'replaces' edge ...,
        // journal#39") is not about it. A missed `replaces` reads as `cites`;
        // a wrong one would tell the reader an entry was superseded.
        let clause_start = lower[..at].rfind(['.', ';', '\n', '(']).map_or(0, |i| i + 1);
        let before: Vec<&str> = lower[clause_start..at]
            .split_whitespace()
            .rev()
            .take(3)
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .collect();
        let said = |verbs: &[&str]| before.iter().any(|w| verbs.contains(w));
        let kind = if said(&["replaces", "supersedes", "corrects", "reverses"]) {
            "replaces"
        } else if said(&["fixes"]) {
            "fixes"
        } else if said(&["implements"]) {
            "implements"
        } else if said(&["verifies"]) {
            "verifies"
        } else {
            "cites"
        };
        let bytes = lower.as_bytes();
        let mut i = at + MARK.len();
        loop {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if let Ok(id) = lower[start..i].parse::<i64>()
                && !out.iter().any(|(o, k)| *o == id && *k == kind) {
                    out.push((id, kind));
                }
            // `/#41`, `, #41`, ` and #41` continue the run.
            let mut j = i;
            while j < bytes.len() && matches!(bytes[j], b'/' | b',' | b' ') {
                j += 1;
            }
            if lower[j..].starts_with("and ") {
                j += 4;
            }
            if j < bytes.len() && bytes[j] == b'#' && bytes.get(j + 1).is_some_and(u8::is_ascii_digit) {
                i = j + 1;
            } else {
                break;
            }
        }
        from = i.max(at + MARK.len());
    }
    out
}

/// Record the references of entry `id`'s text, to entries that exist.
fn insert_links(conn: &Connection, id: i64, body: &str) -> Result<()> {
    let mut stmt = conn.prepare_cached(
        "INSERT OR IGNORE INTO entry_links(from_id, to_id, kind) \
         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM entries WHERE id = ?2)",
    )?;
    for (to, kind) in parse_refs(body) {
        if to != id {
            stmt.execute(params![id, to, kind])?;
        }
    }
    Ok(())
}

/// Journals written before links existed get theirs once, from the text.
fn backfill_links(conn: &mut Connection) -> Result<()> {
    let done: Option<String> =
        conn.query_row("SELECT value FROM journal_meta WHERE key = 'links_parsed'", [], |r| r.get(0)).optional()?;
    if done.is_some() {
        return Ok(());
    }
    let tx = write_tx(conn)?;
    let rows: Vec<(i64, String)> = {
        let mut stmt = tx.prepare("SELECT id, body FROM entries")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (id, body) in rows {
        insert_links(&tx, id, &body)?;
    }
    tx.execute("INSERT OR REPLACE INTO journal_meta(key, value) VALUES ('links_parsed', '1')", [])?;
    tx.commit()?;
    Ok(())
}

/// Marks any entry can carry on top of its category, so an entry found by
/// meaning alone still says how far to trust it.
///
/// - Work items, open until a session closes them (`journal close`): todo,
///   question, problem, `bugreport` (a defect as reported — symptom, steps —
///   not yet worked through), reminder, `unverified` (a hypothesis, closed by
///   checking it), `disputed` (contradicts another entry, closed when
///   settled), `ai-feedback` (an agent's suggestion for the harness itself).
/// - Marks, removed with `unmark`: `specification` (how something must
///   behave: its behaviour, isolation boundaries, policies, invariants — the
///   spec a feature is built and checked against), `constraint` (a rule, a
///   limit, a constant, an invariant to follow, not weigh), `important`,
///   `idea`, `concept`, `comment`, `verified` (a declaration that the check
///   passed — the entry carries it, its `--proof`s point at the evidence; what
///   lets its todo's branch close), `deprecated` (no longer true; ranked last
///   in a search).
///
/// Flags combine freely: one entry can be a specification and a constraint
/// at once (operator, 2026-10-31).
///
/// An open work item, a specification, a constraint or an important mark
/// keeps its entry past retention; the other marks do not (salience decides).
pub const FLAGS: [&str; 16] = [
    "todo",
    "question",
    "problem",
    "bugreport",
    "reminder",
    "unverified",
    "disputed",
    "ai-feedback",
    "specification",
    "constraint",
    "important",
    "idea",
    "concept",
    "comment",
    "verified",
    "deprecated",
];

const MARKS: [&str; 8] =
    ["specification", "constraint", "important", "idea", "concept", "comment", "verified", "deprecated"];

/// Marks that neither hold an entry past retention nor belong in the agenda:
/// history, evidence, and notes kept for what they say.
pub const PASSIVE: [&str; 5] = ["idea", "concept", "comment", "verified", "deprecated"];

/// The marks, for SQL to tell them from work items.
fn marks_json() -> String {
    serde_json::to_string(&MARKS).expect("strings serialise")
}

pub fn closable(flag: &str) -> bool {
    !MARKS.contains(&flag)
}

/// Known flags, lowercased, de-duplicated, in [`FLAGS`] order.
pub fn normalize_flags<S: AsRef<str>>(raw: &[S]) -> Result<Vec<String>> {
    let mut wanted = HashSet::new();
    for f in raw {
        let f = f.as_ref().trim().trim_start_matches('#').to_lowercase();
        if !FLAGS.contains(&f.as_str()) {
            bail!("unknown flag `{f}`: one of {}", FLAGS.join(", "));
        }
        wanted.insert(f);
    }
    Ok(FLAGS.iter().filter(|f| wanted.contains(**f)).map(|f| f.to_string()).collect())
}

/// The file name for a project path. The folder name is only decoration — the
/// hash is what makes it unique — so it is reduced to a safe alphabet.
pub fn journal_file_name(project: &Path) -> String {
    let mut name: String = project
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .take(40)
        .collect();
    if name.trim_matches(|c| c == '.' || c == '-').is_empty() {
        name = "root".into();
    }
    let digest = Sha256::digest(project.to_string_lossy().as_bytes());
    format!("{name}-{}.db", hex(&digest)[..12].to_owned())
}

pub fn journal_path(dir: &Path, project: &Path) -> PathBuf {
    dir.join(journal_file_name(project))
}

/// What a project's journal is filed under: its repository, when it has one
/// (`git:host/owner/repo`, from `project_id::resolve_project_id`), so every
/// checkout of it — on this machine or a laptop joined to this daemon — shares
/// one journal; otherwise its directory, as before.
pub fn project_key(project_id: Option<&str>, path: &Path) -> PathBuf {
    match project_id {
        Some(id) if !id.is_empty() && !id.starts_with("local:") => PathBuf::from(format!("git:{id}")),
        _ => path.to_path_buf(),
    }
}

/// A journal kept under the directory `path` before journals were filed by
/// repository becomes the journal of `key`: same entries, renamed. Nothing
/// happens when `key` already has a journal, or `path` has none.
pub fn adopt_path_journal(dir: &Path, key: &Path, path: &Path) -> Result<bool> {
    if key == path {
        return Ok(false);
    }
    let (from, to) = (journal_path(dir, path), journal_path(dir, key));
    if to.exists() || !from.exists() {
        return Ok(false);
    }
    {
        let conn = open_file(&from)?;
        let recorded: Option<String> = conn
            .query_row("SELECT value FROM journal_meta WHERE key = 'project_path'", [], |r| r.get(0))
            .optional()?;
        if recorded.as_deref() != Some(&*path.to_string_lossy()) {
            return Ok(false);
        }
        // Everything into the main file, so the rename carries it all.
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        conn.execute(
            "UPDATE journal_meta SET value = ?1 WHERE key = 'project_path'",
            params![key.to_string_lossy()],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO journal_meta(key, value) VALUES ('adopted_from', ?1)",
            params![path.to_string_lossy()],
        )?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    std::fs::rename(&from, &to).with_context(|| format!("rename {} -> {}", from.display(), to.display()))?;
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(from.with_extension(ext));
    }
    Ok(true)
}

/// Open a project's journal. `create = false` returns `None` when it has never
/// been written, so a read from an unrelated directory does not leave an empty
/// file behind.
pub fn open(dir: &Path, project: &Path, create: bool) -> Result<Option<Connection>> {
    let path = journal_path(dir, project);
    if !create && !path.exists() {
        return Ok(None);
    }
    create_private_dir(dir)?;
    let conn = open_file(&path)?;
    let recorded: Option<String> = conn
        .query_row("SELECT value FROM journal_meta WHERE key = 'project_path'", [], |r| r.get(0))
        .optional()?;
    match recorded {
        None => {
            conn.execute(
                "INSERT INTO journal_meta(key, value) VALUES ('project_path', ?1)",
                params![project.to_string_lossy()],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO journal_meta(key, value) VALUES ('created_at', ?1)",
                params![now_secs().to_string()],
            )?;
        }
        // Twelve hex digits make a collision a curiosity, but a journal that
        // silently served another project would be exactly the leak this
        // design exists to prevent.
        Some(p) if p != project.to_string_lossy() => {
            bail!("journal {} belongs to {p}, not {}", path.display(), project.display())
        }
        Some(_) => {}
    }
    Ok(Some(conn))
}

fn open_file(path: &Path) -> Result<Connection> {
    crate::ensure_sqlite_vec_registered();
    let fresh = !path.exists();
    let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    if fresh {
        restrict(path);
    }
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    conn.execute_batch(SCHEMA).context("apply journal schema")?;
    conn.execute_batch(crate::mindmap::SCHEMA).context("apply mind-map schema")?;
    crate::mindmap::migrate(&conn).context("migrate the mind-map tables")?;
    let mut conn = conn;
    backfill_links(&mut conn).context("journal links backfill")?;
    Ok(conn)
}

fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Journals hold whatever agents wrote down, which is not for other users.
fn restrict(_path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(_path, std::fs::Permissions::from_mode(0o600));
    }
}

/// Tags and categories share one normal form: a leading `#` dropped, trimmed,
/// lowercased, inner whitespace turned into `-`. Empty means "none".
pub fn normalize_label(raw: &str) -> Option<String> {
    let s = raw.trim();
    let s = s.strip_prefix('#').unwrap_or(s).trim().to_lowercase();
    let s: String = s.split_whitespace().collect::<Vec<_>>().join("-");
    (!s.is_empty()).then_some(s)
}

/// Normalize, drop empties, de-duplicate, keep first-seen order.
pub fn normalize_tags<S: AsRef<str>>(raw: &[S]) -> Vec<String> {
    let mut seen = HashSet::new();
    raw.iter()
        .filter_map(|t| normalize_label(t.as_ref()))
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

pub fn normalize_salience(raw: Option<&str>) -> Result<String> {
    match raw.map(|s| s.trim().to_lowercase()).as_deref() {
        None | Some("") => Ok("medium".into()),
        Some(s @ ("high" | "medium" | "low")) => Ok(s.into()),
        Some(other) => bail!("salience must be high, medium or low, not `{other}`"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct NewEntry {
    pub body: String,
    pub category: String,
    pub tags: Vec<String>,
    pub salience: String,
    pub agent: Option<String>,
    pub agent_id: Option<String>,
    pub session_id: Option<String>,
    pub feature: Option<String>,
    pub source_artifact: Option<String>,
    /// [`FLAGS`] to set, open.
    pub flags: Vec<String>,
    /// Where the evidence is: files, screenshots, logs, URLs (`--proof`).
    pub proofs: Vec<String>,
    /// Bring the entry back to its author every this many seconds until it
    /// is closed (`--every`). Implies the `reminder` flag; needs `remind_to`.
    pub remind_every: Option<i64>,
    /// Who it comes back to: the writing session's nick, and its host.
    pub remind_to: Option<(String, Option<String>)>,
    /// Do not read `journal#N` references from the text: an entry imported
    /// from another journal numbers them in that journal.
    pub foreign: bool,
    /// Entries that have to be closed before this one can be (CON-91).
    pub blocked_by: Vec<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AddStatus {
    Written,
    Deduped,
}

/// Validate and store one entry. The caller has resolved the project; this
/// checks the entry itself.
pub fn add(conn: &mut Connection, entry: &NewEntry, now: i64) -> Result<(AddStatus, i64)> {
    let body = entry.body.trim();
    if body.is_empty() {
        bail!("the entry is empty");
    }
    if body.chars().count() > MAX_BODY_CHARS {
        bail!("the entry is longer than {MAX_BODY_CHARS} characters; a journal entry is a note — link the document instead (--source-artifact)");
    }
    let mut flags = normalize_flags(&entry.flags)?;
    if let Some(every) = entry.remind_every {
        if every < MIN_REMIND_SECS {
            bail!("--every must be at least {} minute(s)", MIN_REMIND_SECS / 60);
        }
        if entry.remind_to.is_none() {
            bail!("a reminder comes back to the session that wrote it: --every needs a session");
        }
        if !flags.iter().any(|f| f == "reminder") {
            flags = normalize_flags(&[flags.clone(), vec!["reminder".to_string()]].concat())?;
        }
    }
    // A todo or a question is often the whole point of the entry; its flag
    // then names it, unless a category says more.
    // Without a category the first flag names the entry; a bug report is a bug.
    let category = normalize_label(&entry.category)
        .or_else(|| flags.first().map(|f| if f == "bugreport" { "bug".to_string() } else { f.clone() }));
    let Some(category) = category else {
        bail!("a category is required (--category), e.g. decision, finding, bug");
    };
    let tags = normalize_tags(&entry.tags);
    if tags.is_empty() {
        bail!("at least one --tag is required: tags are how the entry is found again");
    }
    let salience = normalize_salience(Some(&entry.salience))?;
    let sha = hex(&Sha256::digest(body.as_bytes()));

    // The duplicate check runs inside the write transaction, so two identical
    // adds arriving together cannot both miss it.
    let tx = write_tx(conn)?;
    let existing: Option<i64> = tx
        .query_row(
            "SELECT id FROM entries WHERE sha256 = ?1 AND category = ?2 AND created_at >= ?3 \
             ORDER BY id DESC LIMIT 1",
            params![sha, category, now - DEDUP_WINDOW_SECS],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        // The same note again still teaches us its tags and flags.
        insert_tags(&tx, id, &tags)?;
        insert_flags(&tx, id, &flags, now)?;
        insert_proofs(&tx, id, &entry.proofs)?;
        insert_reminder(&tx, id, entry, now)?;
        for blocker in &entry.blocked_by {
            insert_block(&tx, id, *blocker)?;
        }
        tx.commit()?;
        return Ok((AddStatus::Deduped, id));
    }
    // Before anything is stored: a refused entry leaves nothing behind.
    if !entry.blocked_by.is_empty() && !flags.iter().any(|f| closable(f)) {
        bail!("this entry has nothing open to close, so nothing can block it: give it a work item such as todo");
    }
    for blocker in &entry.blocked_by {
        check_blocker(&tx, *blocker)?;
    }

    tx.execute(
        "INSERT INTO entries (created_at, category, body, sha256, salience, agent, agent_id, \
                              session_id, feature, source_artifact) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            now,
            category,
            body,
            sha,
            salience,
            clean(&entry.agent),
            clean(&entry.agent_id),
            clean(&entry.session_id),
            clean(&entry.feature),
            clean(&entry.source_artifact),
        ],
    )?;
    let id = tx.last_insert_rowid();
    insert_tags(&tx, id, &tags)?;
    insert_flags(&tx, id, &flags, now)?;
    insert_proofs(&tx, id, &entry.proofs)?;
    insert_reminder(&tx, id, entry, now)?;
    if !entry.foreign {
        insert_links(&tx, id, body)?;
    }
    for blocker in &entry.blocked_by {
        insert_block(&tx, id, *blocker)?;
    }
    tx.commit()?;
    Ok((AddStatus::Written, id))
}

/// Link `from` to `to` explicitly — for a reference the text does not make.
/// Returns false when the link was already there.
pub fn link(conn: &mut Connection, from: i64, to: i64, kind: &str) -> Result<bool> {
    if !LINK_KINDS.contains(&kind) {
        bail!("link kind must be one of {}, not `{kind}`", LINK_KINDS.join(", "));
    }
    if from == to {
        bail!("an entry cannot link to itself");
    }
    let tx = write_tx(conn)?;
    for id in [from, to] {
        if !exists(&tx, id)? {
            bail!("no entry {id}");
        }
    }
    let added = tx.execute("INSERT OR IGNORE INTO entry_links(from_id, to_id, kind) VALUES (?1, ?2, ?3)", params![from, to, kind])? > 0;
    tx.commit()?;
    Ok(added)
}

/// A blocker has to exist and have something open: what is closed already holds nothing up, and
/// what has no work item could never release it.
fn check_blocker(conn: &Connection, blocker: i64) -> Result<()> {
    if !exists(conn, blocker)? {
        bail!("no entry {blocker}");
    }
    if open_work_items(conn, blocker)?.is_empty() {
        bail!("entry {blocker} has nothing open: it would hold nothing up, or never let go");
    }
    Ok(())
}

fn insert_block(conn: &Connection, id: i64, blocker: i64) -> Result<bool> {
    if id == blocker {
        bail!("an entry cannot be blocked by itself");
    }
    check_blocker(conn, blocker)?;
    // Whatever the blocker waits for, at any depth: none of them may wait for this one.
    let mut seen = vec![blocker];
    let mut at = 0;
    while at < seen.len() {
        for next in blockers_of(conn, seen[at])? {
            if next == id {
                bail!("entry {blocker} already waits for entry {id}: neither could ever close");
            }
            if !seen.contains(&next) {
                seen.push(next);
            }
        }
        at += 1;
    }
    Ok(conn.execute("INSERT OR IGNORE INTO entry_links(from_id, to_id, kind) VALUES (?1, ?2, ?3)", params![id, blocker, BLOCKED_BY])? > 0)
}

fn blockers_of(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached("SELECT to_id FROM entry_links WHERE from_id = ?1 AND kind = ?2 ORDER BY to_id")?;
    let found = stmt.query_map(params![id, BLOCKED_BY], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(found)
}

/// Those of an entry's blockers that are not closed yet.
fn open_blockers(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut open = Vec::new();
    for blocker in blockers_of(conn, id)? {
        if !open_work_items(conn, blocker)?.is_empty() {
            open.push(blocker);
        }
    }
    Ok(open)
}

/// `id` cannot be closed until `blocker` is (CON-91).
pub fn block(conn: &mut Connection, id: i64, blocker: i64) -> Result<bool> {
    let tx = write_tx(conn)?;
    if !exists(&tx, id)? {
        bail!("no entry {id}");
    }
    if open_work_items(&tx, id)?.is_empty() {
        bail!("entry {id} has nothing open to close, so nothing can block it");
    }
    let added = insert_block(&tx, id, blocker)?;
    tx.commit()?;
    Ok(added)
}

/// Lifts a block. False if there was none.
pub fn unblock(conn: &Connection, id: i64, blocker: i64) -> Result<bool> {
    Ok(unlink(conn, id, blocker, Some(BLOCKED_BY))? > 0)
}

/// Remove links from `from` to `to` — of one kind, or all. Returns how many.
pub fn unlink(conn: &Connection, from: i64, to: i64, kind: Option<&str>) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM entry_links WHERE from_id = ?1 AND to_id = ?2 AND (?3 IS NULL OR kind = ?3)",
        params![from, to, kind],
    )?)
}

/// Set flags; one already set — open or closed — is left as it is.
fn insert_flags(conn: &Connection, id: i64, flags: &[String], now: i64) -> Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO entry_flags(entry_id, flag, set_at) VALUES (?1, ?2, ?3)")?;
    for flag in flags {
        stmt.execute(params![id, flag, now])?;
    }
    Ok(())
}

/// Shortest reminder interval: the daemon looks for due reminders every 30 s.
pub const MIN_REMIND_SECS: i64 = 60;

fn insert_reminder(conn: &Connection, id: i64, entry: &NewEntry, now: i64) -> Result<()> {
    let (Some(every), Some((nick, host))) = (entry.remind_every, entry.remind_to.as_ref()) else { return Ok(()) };
    conn.execute(
        "INSERT OR REPLACE INTO entry_reminders(entry_id, every_secs, next_at, nick, host) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, every, now + every, nick, host],
    )?;
    Ok(())
}

/// A reminder whose time has come.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueReminder {
    pub id: i64,
    pub body: String,
    pub every_secs: i64,
    pub nick: String,
    pub host: Option<String>,
}

/// Reminders due at `now`: an open `reminder` flag and `next_at` reached. A
/// closed one stays in the table, silent, so `reopen` brings it back.
pub fn due_reminders(conn: &Connection, now: i64) -> Result<Vec<DueReminder>> {
    let mut stmt = conn.prepare_cached(
        "SELECT r.entry_id, e.body, r.every_secs, r.nick, r.host FROM entry_reminders r \
         JOIN entries e ON e.id = r.entry_id \
         JOIN entry_flags f ON f.entry_id = r.entry_id AND f.flag = 'reminder' AND f.closed_at IS NULL \
         WHERE r.next_at <= ?1 ORDER BY r.next_at",
    )?;
    let rows = stmt.query_map(params![now], |r| {
        Ok(DueReminder { id: r.get(0)?, body: r.get(1)?, every_secs: r.get(2)?, nick: r.get(3)?, host: r.get(4)? })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The reminder reached its author: next time is one interval from now, not
/// from when it was due, so one that waited out an absence does not burst.
pub fn reminder_sent(conn: &Connection, id: i64, now: i64) -> Result<()> {
    conn.execute("UPDATE entry_reminders SET next_at = ?2 + every_secs WHERE entry_id = ?1", params![id, now])?;
    Ok(())
}

fn reminder_of(conn: &Connection, id: i64) -> Result<Option<Reminder>> {
    Ok(conn
        .query_row(
            "SELECT every_secs, next_at, nick FROM entry_reminders WHERE entry_id = ?1",
            params![id],
            |r| Ok(Reminder { every_secs: r.get(0)?, next_at: r.get(1)?, nick: r.get(2)? }),
        )
        .optional()?)
}

fn insert_proofs(conn: &Connection, id: i64, proofs: &[String]) -> Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO entry_proofs(entry_id, proof) VALUES (?1, ?2)")?;
    for p in proofs.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        stmt.execute(params![id, p])?;
    }
    Ok(())
}

/// Text an evidence entry must carry beyond its `journal#N` references: what
/// was checked, how, under which conditions, how many times. Short enough
/// for one honest sentence, long enough to refuse "Verifies journal#5: ok".
pub const MIN_EVIDENCE_CHARS: usize = 40;

/// Project policy: evidence must point at proofs (`--proof`: screenshots,
/// logs, files, URLs), not only describe them. Kept in the journal, so every
/// checkout and machine sharing it follows the same rule.
const EVIDENCE_FIRST_KEY: &str = "policy.evidence_first";

pub fn evidence_first(conn: &Connection) -> Result<bool> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM journal_meta WHERE key = ?1", params![EVIDENCE_FIRST_KEY], |r| r.get(0))
        .optional()?;
    Ok(v.as_deref() == Some("1"))
}

pub fn set_evidence_first(conn: &Connection, on: bool) -> Result<()> {
    if on {
        conn.execute(
            "INSERT OR REPLACE INTO journal_meta(key, value) VALUES (?1, '1')",
            params![EVIDENCE_FIRST_KEY],
        )?;
    } else {
        conn.execute("DELETE FROM journal_meta WHERE key = ?1", params![EVIDENCE_FIRST_KEY])?;
    }
    Ok(())
}

/// Why entry `e` does not count as evidence, or `None` when it does: a
/// separate, live entry describing the check in its own words — and, under
/// evidence-first, pointing at at least one proof.
fn evidence_gap(conn: &Connection, e: i64) -> Result<Option<String>> {
    if has_open_flag(conn, e, "deprecated")? {
        return Ok(Some(format!("#{e} is deprecated")));
    }
    let body: String = conn.query_row("SELECT body FROM entries WHERE id = ?1", params![e], |r| r.get(0))?;
    let mut text = body.to_lowercase();
    for (id, _) in parse_refs(&body) {
        text = text.replace(&format!("journal#{id}"), "").replace(&format!("#{id}"), "");
    }
    let said = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().count();
    if said < MIN_EVIDENCE_CHARS {
        return Ok(Some(format!(
            "#{e} says too little ({said} characters besides its references; {MIN_EVIDENCE_CHARS} at least)"
        )));
    }
    if evidence_first(conn)? && proofs_of(conn, e)?.is_empty() {
        return Ok(Some(format!("#{e} points at no proof, and this project is evidence-first")));
    }
    Ok(None)
}

/// The entry that proves `id`: one that `verifies` it and is evidence by
/// [`evidence_gap`]. `Err` explains what is missing.
fn proof_of(conn: &Connection, id: i64) -> Result<std::result::Result<i64, String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT from_id FROM entry_links WHERE to_id = ?1 AND kind = 'verifies' AND from_id != ?1 ORDER BY from_id",
    )?;
    let candidates: Vec<i64> = stmt.query_map(params![id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut gaps = Vec::new();
    for c in candidates {
        match evidence_gap(conn, c)? {
            None => return Ok(Ok(c)),
            Some(g) => gaps.push(g),
        }
    }
    Ok(Err(gaps.join("; ")))
}

fn evidence_howto(conn: &Connection, id: i64) -> Result<String> {
    let proofs = if evidence_first(conn)? {
        " This project is evidence-first: add --proof <file|screenshot|log|url> (repeatable) to that entry."
    } else {
        " Add --proof <file|url> when there is something to point at."
    };
    Ok(format!(
        "Add a separate entry: journal add \"Verifies journal#{id}: <what was checked, how, \
         under which conditions, how many times, the result>\" --category verification.{proofs}"
    ))
}

/// The gate on `unverified`: it is lifted only by a separate entry carrying
/// the evidence (operator constraint 2026-10-29).
fn require_proof(conn: &Connection, id: i64) -> Result<i64> {
    match proof_of(conn, id)? {
        Ok(e) => Ok(e),
        Err(gaps) => {
            let why = if gaps.is_empty() { "no entry verifies it".to_string() } else { gaps };
            bail!("#{id} stays unverified: {why}. {}", evidence_howto(conn, id)?)
        }
    }
}

fn exists(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM entries WHERE id = ?1", params![id], |_| Ok(())).optional()?.is_some())
}

/// Flag an existing entry. Returns the flags that were not set before.
pub fn mark(conn: &mut Connection, id: i64, flags: &[String], now: i64) -> Result<Vec<String>> {
    let flags = normalize_flags(flags)?;
    if flags.is_empty() {
        bail!("name at least one flag: {}", FLAGS.join(", "));
    }
    let tx = write_tx(conn)?;
    if !exists(&tx, id)? {
        bail!("no entry {id}");
    }
    let mut added = Vec::new();
    for flag in flags {
        if tx.execute("INSERT OR IGNORE INTO entry_flags(entry_id, flag, set_at) VALUES (?1, ?2, ?3)", params![id, flag, now])? > 0 {
            added.push(flag);
        }
    }
    tx.commit()?;
    Ok(added)
}

/// Remove flags from an entry, open or closed. Returns those removed.
pub fn unmark(conn: &mut Connection, id: i64, flags: &[String]) -> Result<Vec<String>> {
    let flags = normalize_flags(flags)?;
    if flags.is_empty() {
        bail!("name at least one flag: {}", FLAGS.join(", "));
    }
    let tx = write_tx(conn)?;
    if !exists(&tx, id)? {
        bail!("no entry {id}");
    }
    if flags.iter().any(|f| f == "unverified") && has_open_flag(&tx, id, "unverified")? {
        require_proof(&tx, id)?;
    }
    let mut removed = Vec::new();
    for flag in flags {
        if tx.execute("DELETE FROM entry_flags WHERE entry_id = ?1 AND flag = ?2", params![id, flag])? > 0 {
            removed.push(flag);
        }
    }
    tx.commit()?;
    Ok(removed)
}

/// Close open work items of an entry — the named ones, or every open one when
/// none is named — recording who closed them and how. Returns those closed.
pub fn close(
    conn: &mut Connection,
    id: i64,
    flags: &[String],
    by: Option<&str>,
    resolution: Option<&str>,
    now: i64,
) -> Result<Vec<String>> {
    close_with(conn, id, flags, by, resolution, now, CloseMode::Checked)
}

/// How a close treats a todo's branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseMode {
    /// A todo closes only once its branch holds a `verified` entry, and then
    /// takes every open work item of the branch with it.
    Checked,
    /// The work was dropped, not done: the branch closes without a verified
    /// entry, and a resolution saying why is required.
    Abandon,
    /// History coming back (`journal import`): just the named flags, no gate.
    Restore,
}

pub fn close_with(
    conn: &mut Connection,
    id: i64,
    flags: &[String],
    by: Option<&str>,
    resolution: Option<&str>,
    now: i64,
    mode: CloseMode,
) -> Result<Vec<String>> {
    let named = normalize_flags(flags)?;
    if let Some(f) = named.iter().find(|f| !closable(f)) {
        bail!("`{f}` is a mark, not a work item: unmark it instead");
    }
    let resolution = resolution.map(str::trim).filter(|r| !r.is_empty());
    let by = by.map(str::trim).filter(|b| !b.is_empty());
    let tx = write_tx(conn)?;
    if !exists(&tx, id)? {
        bail!("no entry {id}");
    }
    let open = open_work_items(&tx, id)?;
    let targets: Vec<String> = if named.is_empty() { open.clone() } else { named };
    if let Some(f) = targets.iter().find(|f| !open.contains(f)) {
        bail!("entry {id} has no open `{f}`");
    }
    if targets.is_empty() {
        bail!("entry {id} has nothing open to close");
    }

    let mut resolution = resolution.map(str::to_owned);
    if targets.iter().any(|f| f == "unverified") {
        match mode {
            CloseMode::Checked => {
                let e = require_proof(&tx, id)?;
                if resolution.is_none() {
                    resolution = Some(format!("verified: journal#{e}"));
                }
            }
            CloseMode::Abandon if resolution.is_none() => {
                bail!("--abandon needs --resolution \"<why it will not be checked>\"")
            }
            // A todo's branch prefixes it below.
            CloseMode::Abandon if !targets.iter().any(|f| f == "todo") => {
                resolution = resolution.map(|r| format!("abandoned: {r}"));
            }
            _ => {}
        }
    }

    // A todo is the root of a branch: everything that implements, verifies,
    // fixes or cites it, downstream. It closes as one, and only on evidence.
    let mut also: Vec<i64> = Vec::new();
    if targets.iter().any(|f| f == "todo") && mode != CloseMode::Restore {
        let branch = branch_ids(&tx, id)?;
        match mode {
            CloseMode::Checked => {
                let verified = branch.iter().copied().find(|e| {
                    *e != id
                        && has_open_flag(&tx, *e, "verified").unwrap_or(false)
                        && matches!(evidence_gap(&tx, *e), Ok(None))
                });
                let Some(v) = verified else {
                    bail!(
                        "todo {id} cannot close yet: its branch has no entry flagged --verified. \
                         Add one that cites the implementation (\"Verifies journal#<id>\") and carries the evidence \
                         that it works — what was checked, how, under which conditions, how many times, the output \
                         ({MIN_EVIDENCE_CHARS}+ characters; under evidence-first also --proof <file|url>) — then close #{id}. \
                         If the work was dropped instead, close it with --abandon --resolution \"<why>\"."
                    );
                };
                if resolution.is_none() {
                    resolution = Some(format!("verified: journal#{v}"));
                }
            }
            CloseMode::Abandon => {
                if resolution.is_none() {
                    bail!("--abandon needs --resolution \"<why the work was dropped>\"");
                }
                resolution = resolution.map(|r| format!("abandoned: {r}"));
            }
            CloseMode::Restore => unreachable!(),
        }
        also = branch.into_iter().filter(|e| *e != id).collect();
    }

    // Dropped work waits for nothing, and history comes back as it was.
    if mode == CloseMode::Checked {
        for e in std::iter::once(id).chain(also.iter().copied()) {
            let waiting = open_blockers(&tx, e)?;
            if !waiting.is_empty() {
                let names = waiting.iter().map(|b| format!("#{b}")).collect::<Vec<_>>().join(", ");
                let whose = if e == id { format!("entry {id}") } else { format!("entry {id}'s branch: entry {e}") };
                bail!("{whose} cannot close yet: blocked by {names}, not closed. Close that first, or lift the block with journal_mark.");
            }
        }
    }

    // How a dropped entry is told from a solved one, whichever work item it was.
    if mode == CloseMode::Abandon && !resolution.as_deref().is_some_and(|r| r.starts_with(ABANDONED)) {
        let Some(why) = resolution.take() else { bail!("--abandon needs --resolution \"<why the work was dropped>\"") };
        resolution = Some(format!("{ABANDONED} {why}"));
    }

    let mut closed = Vec::new();
    for flag in &targets {
        tx.execute(
            "UPDATE entry_flags SET closed_at = ?1, closed_by = ?2, resolution = ?3 WHERE entry_id = ?4 AND flag = ?5",
            params![now, by, resolution, id, flag],
        )?;
        closed.push(flag.clone());
    }
    for e in also {
        for flag in open_work_items(&tx, e)? {
            // A hypothesis in the branch is lifted only by its own evidence;
            // without it, it stays open for the agenda to show.
            if flag == "unverified" && mode == CloseMode::Checked && proof_of(&tx, e)?.is_err() {
                continue;
            }
            tx.execute(
                "UPDATE entry_flags SET closed_at = ?1, closed_by = ?2, resolution = ?3 WHERE entry_id = ?4 AND flag = ?5",
                params![now, by, resolution, e, flag],
            )?;
            closed.push(format!("#{e} {flag}"));
        }
    }
    tx.commit()?;
    Ok(closed)
}

fn open_work_items(conn: &Connection, id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT flag FROM entry_flags WHERE entry_id = ?1 AND closed_at IS NULL")?;
    let rows = stmt.query_map(params![id], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.into_iter().filter(|f| closable(f)).collect())
}

fn has_open_flag(conn: &Connection, id: i64, flag: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM entry_flags WHERE entry_id = ?1 AND flag = ?2 AND closed_at IS NULL",
            params![id, flag],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// Most entries a branch walk visits: a branch is a piece of work, not the
/// whole journal, and a cycle of links must not make it run away.
const BRANCH_LIMIT: usize = 200;

/// The links that make a branch: an implementation implements its todo, the
/// evidence verifies the implementation, a follow-up fixes it. Every other link
/// (cites, replaces) connects branches into the graph without pulling one
/// into another — or closing one todo would close another's work.
const STRUCTURAL: [&str; 3] = ["implements", "verifies", "fixes"];

fn structural_sql(col: &str) -> String {
    format!("{col} IN ({})", STRUCTURAL.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(","))
}

fn is_todo(conn: &Connection, e: i64) -> Result<bool> {
    Ok(conn.query_row("SELECT 1 FROM entry_flags WHERE entry_id = ?1 AND flag = 'todo'", params![e], |_| Ok(())).optional()?.is_some())
}

/// `root` and the entries that belong to its branch: reached downstream over
/// structural links, each one hanging from `root` rather than from another
/// todo (an entry that implements todo B and fixes something in A's branch is
/// B's). In the order reached.
fn branch_ids(conn: &Connection, root: i64) -> Result<Vec<i64>> {
    let mut seen = vec![root];
    let mut queue = std::collections::VecDeque::from([root]);
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT from_id FROM entry_links WHERE to_id = ?1 AND {} ORDER BY from_id",
        structural_sql("kind")
    ))?;
    while let Some(at) = queue.pop_front() {
        let children: Vec<i64> = stmt.query_map(params![at], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for c in children {
            if seen.contains(&c) || seen.len() >= BRANCH_LIMIT || is_todo(conn, c)? {
                continue;
            }
            if branch_root(conn, c)? == Some(root) {
                seen.push(c);
                queue.push_back(c);
            }
        }
    }
    Ok(seen)
}

/// The todo a branch hangs from: `id` itself when it is a todo, else the
/// nearest todo upstream over structural links (breadth first, so the
/// closest one wins).
fn branch_root(conn: &Connection, id: i64) -> Result<Option<i64>> {
    let mut seen = vec![id];
    let mut queue = std::collections::VecDeque::from([id]);
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT to_id FROM entry_links WHERE from_id = ?1 AND {} ORDER BY to_id DESC",
        structural_sql("kind")
    ))?;
    while let Some(at) = queue.pop_front() {
        if is_todo(conn, at)? {
            return Ok(Some(at));
        }
        let parents: Vec<i64> = stmt.query_map(params![at], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for p in parents {
            if !seen.contains(&p) && seen.len() < BRANCH_LIMIT {
                seen.push(p);
                queue.push_back(p);
            }
        }
    }
    Ok(None)
}

/// A todo and the work under it.
#[derive(Debug, Clone, Serialize)]
pub struct Branch {
    pub root: i64,
    /// `open` (work under way), `verified` (a verified entry is in, the todo
    /// can close), `closed`.
    pub state: &'static str,
    /// The verified entry, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verified: Option<i64>,
    /// Root first, then downstream, each with the entry it hangs from.
    pub entries: Vec<BranchEntry>,
    /// Other branches this one is linked with (any link, either way, from any
    /// of its entries): the graph between pieces of work.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linked_branches: Vec<LinkedBranch>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkedBranch {
    /// That branch's todo.
    pub root: i64,
    /// The link that connects them, read from this branch: `#12 cites #40`.
    pub via: String,
    pub todo: Brief,
}

#[derive(Debug, Clone, Serialize)]
pub struct BranchEntry {
    /// The entry this one links to within the branch (`None` for the root).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    pub entry: Brief,
}

/// The branch `id` belongs to: from its todo down.
pub fn branch(conn: &Connection, id: i64) -> Result<Option<Branch>> {
    let Some(root) = branch_root(conn, id)? else { return Ok(None) };
    let ids = branch_ids(conn, root)?;
    let mut entries = Vec::new();
    let mut verified = None;
    for e in &ids {
        let parent: Option<(i64, String)> = if *e == root {
            None
        } else {
            conn.query_row(
                &format!(
                    "SELECT to_id, kind FROM entry_links WHERE from_id = ?1 AND to_id IN ({}) ORDER BY to_id DESC LIMIT 1",
                    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
                ),
                params![e],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        };
        if *e != root && verified.is_none() && has_open_flag(conn, *e, "verified")? {
            verified = Some(*e);
        }
        if let Some(b) = brief(conn, *e)? {
            entries.push(BranchEntry { parent: parent.as_ref().map(|p| p.0), via: parent.map(|p| p.1), entry: b });
        }
    }
    let mut linked_branches: Vec<LinkedBranch> = Vec::new();
    for e in &ids {
        for l in links_of(conn, *e)? {
            if ids.contains(&l.id) {
                continue;
            }
            let Some(other) = branch_root(conn, l.id)? else { continue };
            if other == root || linked_branches.iter().any(|b| b.root == other) {
                continue;
            }
            let via = if l.dir == "out" { format!("#{e} {} #{}", l.kind, l.id) } else { format!("#{} {} #{e}", l.id, l.kind) };
            if let Some(todo) = brief(conn, other)? {
                linked_branches.push(LinkedBranch { root: other, via, todo });
            }
        }
    }
    let state = if !has_open_flag(conn, root, "todo")? {
        "closed"
    } else if verified.is_some() {
        "verified"
    } else {
        "open"
    };
    Ok(Some(Branch { root, state, verified, entries, linked_branches }))
}

/// Open closed work items again — the named ones, or every closed one.
pub fn reopen(conn: &mut Connection, id: i64, flags: &[String]) -> Result<Vec<String>> {
    let named = normalize_flags(flags)?;
    let tx = write_tx(conn)?;
    if !exists(&tx, id)? {
        bail!("no entry {id}");
    }
    let closed: Vec<String> = {
        let mut stmt = tx.prepare("SELECT flag FROM entry_flags WHERE entry_id = ?1 AND closed_at IS NOT NULL")?;
        let rows = stmt.query_map(params![id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let targets: Vec<String> = if named.is_empty() { closed.clone() } else { named };
    if let Some(f) = targets.iter().find(|f| !closed.contains(f)) {
        bail!("entry {id} has no closed `{f}`");
    }
    if targets.is_empty() {
        bail!("entry {id} has nothing closed to reopen");
    }
    for flag in &targets {
        tx.execute(
            "UPDATE entry_flags SET closed_at = NULL, closed_by = NULL, resolution = NULL WHERE entry_id = ?1 AND flag = ?2",
            params![id, flag],
        )?;
    }
    tx.commit()?;
    Ok(targets)
}

/// Every multi-statement write starts as `BEGIN IMMEDIATE`. A deferred
/// transaction takes the write lock only at its first write, and in WAL mode
/// a writer that finds the lock held by another connection gets SQLITE_BUSY
/// at once, without `busy_timeout` ever waiting: a burst of `journal add`
/// while the daemon stored vectors answered "database is locked". Taking the
/// lock up front is what lets the busy timeout queue the writers.
fn write_tx(conn: &mut Connection) -> Result<Transaction<'_>> {
    Ok(conn.transaction_with_behavior(TransactionBehavior::Immediate)?)
}

fn clean(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

fn insert_tags(conn: &Connection, id: i64, tags: &[String]) -> Result<()> {
    let mut stmt = conn.prepare_cached("INSERT OR IGNORE INTO entry_tags(entry_id, tag) VALUES (?1, ?2)")?;
    for tag in tags {
        stmt.execute(params![id, tag])?;
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub category: Option<String>,
    /// Any of these (OR): `--tag nginx --tag docker` finds either.
    pub tags: Vec<String>,
    pub agent: Option<String>,
    pub salience: Option<String>,
    pub feature: Option<String>,
    /// Only entries created at or after this unix second.
    pub since: Option<i64>,
    /// Only entries carrying ANY of these [`FLAGS`].
    pub flags: Vec<String>,
    /// With `Some`, only entries whose (named) flag is open (`true`) or
    /// closed (`false`).
    pub open: Option<bool>,
    /// Only what can be worked on now: leaves out entries waiting for an open blocker.
    pub not_blocked: bool,
}

impl Filter {
    /// The filter as SQL over `entries e`, with its parameters.
    fn sql(&self) -> (String, Vec<rusqlite::types::Value>) {
        use rusqlite::types::Value;
        let mut clauses = Vec::new();
        let mut values = Vec::new();
        let eq = |col: &str, v: &Option<String>, clauses: &mut Vec<String>, values: &mut Vec<Value>| {
            if let Some(v) = v {
                clauses.push(format!("e.{col} = ?"));
                values.push(Value::Text(v.clone()));
            }
        };
        eq("category", &self.category.as_deref().and_then(normalize_label), &mut clauses, &mut values);
        eq("agent", &self.agent, &mut clauses, &mut values);
        eq("salience", &self.salience, &mut clauses, &mut values);
        eq("feature", &self.feature, &mut clauses, &mut values);
        if let Some(since) = self.since {
            clauses.push("e.created_at >= ?".into());
            values.push(Value::Integer(since));
        }
        // Unknown flags were refused where the filter was built.
        let flags = normalize_flags(&self.flags).unwrap_or_default();
        if !flags.is_empty() || self.open.is_some() {
            let mut sub = String::from("SELECT entry_id FROM entry_flags WHERE 1");
            if !flags.is_empty() {
                sub.push_str(&format!(" AND flag IN ({})", vec!["?"; flags.len()].join(",")));
                values.extend(flags.into_iter().map(Value::Text));
            }
            match self.open {
                Some(true) => sub.push_str(" AND closed_at IS NULL"),
                Some(false) => sub.push_str(" AND closed_at IS NOT NULL"),
                None => {}
            }
            clauses.push(format!("e.id IN ({sub})"));
        }
        if self.not_blocked {
            let open = OPEN_ITEM_SQL.replace("{}", "l.to_id");
            clauses.push(format!("NOT EXISTS (SELECT 1 FROM entry_links l WHERE l.from_id = e.id AND l.kind = '{BLOCKED_BY}' AND EXISTS ({open}))"));
            values.push(Value::Text(marks_json()));
        }
        let tags = normalize_tags(&self.tags);
        if !tags.is_empty() {
            clauses.push(format!(
                "e.id IN (SELECT entry_id FROM entry_tags WHERE tag IN ({}))",
                vec!["?"; tags.len()].join(",")
            ));
            values.extend(tags.into_iter().map(Value::Text));
        }
        if clauses.is_empty() {
            ("1".into(), values)
        } else {
            (clauses.join(" AND "), values)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: i64,
    /// When the entry was written, unix seconds; for an imported insight,
    /// when the insight was.
    pub created_at: i64,
    /// `created_at` as RFC 3339 UTC, for readers that should not convert.
    pub created: String,
    pub category: String,
    pub tags: Vec<String>,
    pub salience: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_artifact: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<FlagState>,
    /// References to and from other entries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    /// Where the evidence this entry carries is: files, screenshots, URLs.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remind: Option<Reminder>,
    /// What has to close before this one can, of what it was blocked by: those still open.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<i64>,
    /// Of what it was blocked by, what was dropped instead of solved: to be re-read.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub blocker_dropped: Vec<i64>,
}

/// An entry that comes back to its author while its `reminder` is open.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Reminder {
    pub every_secs: i64,
    pub next_at: i64,
    pub nick: String,
}

/// One edge as seen from an entry: `out` — this entry `kind`s `id` (cites,
/// replaces, fixes); `in` — entry `id` `kind`s this one (cited by, ...).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Link {
    pub kind: String,
    pub dir: &'static str,
    pub id: i64,
}

/// Another entry in brief, as it appears next to the one being read.
#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub id: i64,
    pub created: String,
    pub category: String,
    /// Flags as words, closed ones suffixed: `deprecated`, `todo (closed)`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Related {
    /// The entries this one links with, in the order of its `links`.
    pub linked: Vec<(Link, Brief)>,
    /// Closest by meaning (cosine similarity, from the stored vectors), not
    /// already linked.
    pub similar: Vec<(f64, Brief)>,
    /// Same `source_artifact` (file, commit, URL).
    pub same_source: Vec<Brief>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FlagState {
    pub flag: String,
    pub open: bool,
    pub set_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

const ENTRY_COLUMNS: &str = "e.id, e.created_at, e.category, e.salience, e.body, e.agent, \
                             e.agent_id, e.session_id, e.feature, e.source_artifact";

fn entry_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Entry> {
    Ok(Entry {
        id: r.get(0)?,
        created_at: r.get(1)?,
        created: rfc3339(r.get(1)?),
        category: r.get(2)?,
        salience: r.get(3)?,
        body: r.get(4)?,
        agent: r.get(5)?,
        agent_id: r.get(6)?,
        session_id: r.get(7)?,
        feature: r.get(8)?,
        source_artifact: r.get(9)?,
        tags: Vec::new(),
        flags: Vec::new(),
        links: Vec::new(),
        proofs: Vec::new(),
        remind: None,
        blocked_by: Vec::new(),
        blocker_dropped: Vec::new(),
    })
}

pub fn rfc3339(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_default()
}

fn attach_tags(conn: &Connection, entries: &mut [Entry]) -> Result<()> {
    let mut stmt = conn.prepare_cached("SELECT tag FROM entry_tags WHERE entry_id = ?1 ORDER BY rowid")?;
    let mut flags = conn.prepare_cached(
        "SELECT flag, set_at, closed_at, closed_by, resolution FROM entry_flags WHERE entry_id = ?1",
    )?;
    for e in entries.iter_mut() {
        e.tags = stmt.query_map(params![e.id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        let mut set: Vec<FlagState> = flags
            .query_map(params![e.id], |r| {
                let closed_at: Option<i64> = r.get(2)?;
                Ok(FlagState {
                    flag: r.get(0)?,
                    open: closed_at.is_none(),
                    set_at: r.get(1)?,
                    closed_at,
                    closed_by: r.get(3)?,
                    resolution: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        set.sort_by_key(|f| FLAGS.iter().position(|k| *k == f.flag));
        e.flags = set;
        e.links = links_of(conn, e.id)?;
        e.blocked_by = open_blockers(conn, e.id)?;
        e.blocker_dropped = dropped_blockers(conn, e.id)?;
        e.proofs = proofs_of(conn, e.id)?;
        e.remind = reminder_of(conn, e.id)?;
    }
    Ok(())
}

fn proofs_of(conn: &Connection, id: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT proof FROM entry_proofs WHERE entry_id = ?1 ORDER BY rowid")?;
    let rows = stmt.query_map(params![id], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn links_of(conn: &Connection, id: i64) -> Result<Vec<Link>> {
    let mut stmt = conn.prepare_cached(
        "SELECT kind, 'out', to_id FROM entry_links WHERE from_id = ?1 \
         UNION ALL SELECT kind, 'in', from_id FROM entry_links WHERE to_id = ?1",
    )?;
    let mut links: Vec<Link> = stmt
        .query_map(params![id], |r| {
            let dir: String = r.get(1)?;
            Ok(Link { kind: r.get(0)?, dir: if dir == "out" { "out" } else { "in" }, id: r.get(2)? })
        })?
        .collect::<rusqlite::Result<_>>()?;
    // What replaces this entry first: it is the one thing a reader must not miss.
    let rank = |l: &Link| match (l.kind.as_str(), l.dir) {
        ("replaces", "in") => 0,
        ("replaces", _) => 1,
        ("fixes", _) => 2,
        _ => 3,
    };
    links.sort_by_key(|l| (rank(l), l.dir, l.id));
    Ok(links)
}

fn brief(conn: &Connection, id: i64) -> Result<Option<Brief>> {
    let Some(e) = get(conn, id)? else { return Ok(None) };
    let flags = e.flags.iter().map(|f| if f.open { f.flag.clone() } else { format!("{} (closed)", f.flag) }).collect();
    let flat = e.body.split_whitespace().collect::<Vec<_>>().join(" ");
    let excerpt = if flat.chars().count() > 160 { format!("{}…", flat.chars().take(160).collect::<String>()) } else { flat };
    Ok(Some(Brief { id: e.id, created: e.created, category: e.category, flags, excerpt }))
}

/// What entry `id` intersects: its links, the `similar` entries closest to it
/// by meaning, and those from the same source. Similarity needs the entry's
/// vector; until the daemon has embedded it, `similar` is empty.
pub fn related(conn: &Connection, id: i64, similar: usize) -> Result<Related> {
    let links = links_of(conn, id)?;
    let mut linked = Vec::new();
    for l in &links {
        if let Some(b) = brief(conn, l.id)? {
            linked.push((l.clone(), b));
        }
    }
    let mut near = Vec::new();
    let vector: Option<Vec<u8>> =
        conn.query_row("SELECT embedding FROM entries_vec WHERE rowid = ?1", params![id], |r| r.get(0)).optional()?;
    if let (Some(v), true) = (vector, similar > 0) {
        let mut stmt = conn.prepare("SELECT rowid, distance FROM entries_vec WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance")?;
        let k = (similar + links.len() + 1) as i64;
        let hits: Vec<(i64, f64)> = stmt.query_map(params![v, k], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
        for (other, distance) in hits {
            if other == id || links.iter().any(|l| l.id == other) {
                continue;
            }
            // e5 vectors are unit length: cosine = 1 - d^2 / 2.
            if let Some(b) = brief(conn, other)? {
                near.push((1.0 - distance * distance / 2.0, b));
            }
            if near.len() == similar {
                break;
            }
        }
    }
    let mut same_source = Vec::new();
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT o.id FROM entries e JOIN entries o ON o.source_artifact = e.source_artifact AND o.id != e.id \
             WHERE e.id = ?1 AND e.source_artifact IS NOT NULL ORDER BY o.created_at DESC LIMIT 5",
        )?;
        let rows = stmt.query_map(params![id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for other in ids {
        if let Some(b) = brief(conn, other)? {
            same_source.push(b);
        }
    }
    Ok(Related { linked, similar: near, same_source })
}

/// What is still open — work items, constraints, important entries — flag by
/// flag in [`FLAGS`] order, newest first: the count and up to `limit` entries
/// each. Flags with nothing open are left out.
pub fn agenda(conn: &Connection, limit: usize) -> Result<Vec<(String, i64, Vec<Entry>)>> {
    let mut out = Vec::new();
    // Evidence, history and plain notes are not "open": the passive marks stay out.
    for flag in FLAGS.iter().filter(|f| !PASSIVE.contains(f)) {
        // What waits for something is not there to be taken: it has its own group below.
        let filter = Filter { flags: vec![flag.to_string()], open: Some(true), not_blocked: closable(flag), ..Default::default() };
        let (total, entries) = list(conn, &filter, 0, limit)?;
        if total > 0 {
            out.push((flag.to_string(), total, entries));
        }
    }
    let waiting = open_entries_with_blocker(conn)?;
    let mut group = |name: &str, ids: Vec<i64>| -> Result<()> {
        if !ids.is_empty() {
            let total = ids.len() as i64;
            let entries = ids.into_iter().take(limit.max(1)).map(|id| get(conn, id)).collect::<Result<Vec<_>>>()?.into_iter().flatten().collect();
            out.push((name.to_string(), total, entries));
        }
        Ok(())
    };
    let mut blocked = Vec::new();
    let mut dropped = Vec::new();
    for id in waiting {
        if !open_blockers(conn, id)?.is_empty() {
            blocked.push(id);
        }
        if !dropped_blockers(conn, id)?.is_empty() {
            dropped.push(id);
        }
    }
    group("blocked", blocked)?;
    group("blocker_dropped", dropped)?;
    Ok(out)
}

/// Entries with something open and a block on them, newest first.
fn open_entries_with_blocker(conn: &Connection) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare_cached("SELECT DISTINCT from_id FROM entry_links WHERE kind = ?1 ORDER BY from_id DESC")?;
    let ids: Vec<i64> = stmt.query_map(params![BLOCKED_BY], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    let mut open = Vec::new();
    for id in ids {
        if !open_work_items(conn, id)?.is_empty() {
            open.push(id);
        }
    }
    Ok(open)
}

/// The blockers of an open entry that were dropped, not solved: it no longer waits for them, and
/// what it waited on is still not there. Stays until the link is lifted or the entry closes.
fn dropped_blockers(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut dropped = Vec::new();
    if open_work_items(conn, id)?.is_empty() {
        return Ok(dropped);
    }
    for blocker in blockers_of(conn, id)? {
        let abandoned = conn
            .query_row(
                "SELECT 1 FROM entry_flags WHERE entry_id = ?1 AND closed_at IS NOT NULL AND resolution LIKE ?2 LIMIT 1",
                params![blocker, format!("{ABANDONED}%")],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if abandoned && open_work_items(conn, blocker)?.is_empty() {
            dropped.push(blocker);
        }
    }
    Ok(dropped)
}

/// What closing `id` set free: open entries that waited for it or for its branch, with no open
/// blocker left. Asked right after the close.
pub fn released_by(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let closing = if is_todo(conn, id)? { branch_ids(conn, id)? } else { vec![id] };
    let mut released = Vec::new();
    let mut stmt = conn.prepare_cached("SELECT from_id FROM entry_links WHERE to_id = ?1 AND kind = ?2 ORDER BY from_id")?;
    for closed in closing {
        let waiting: Vec<i64> = stmt.query_map(params![closed, BLOCKED_BY], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        for e in waiting {
            if !released.contains(&e) && !open_work_items(conn, e)?.is_empty() && open_blockers(conn, e)?.is_empty() {
                released.push(e);
            }
        }
    }
    Ok(released)
}

pub fn get(conn: &Connection, id: i64) -> Result<Option<Entry>> {
    let mut entry = conn
        .query_row(&format!("SELECT {ENTRY_COLUMNS} FROM entries e WHERE e.id = ?1"), params![id], entry_from_row)
        .optional()?;
    if let Some(e) = entry.as_mut() {
        attach_tags(conn, std::slice::from_mut(e))?;
    }
    Ok(entry)
}

/// Newest first. Returns the number of matching entries and one page of them.
pub fn list(conn: &Connection, filter: &Filter, offset: usize, limit: usize) -> Result<(i64, Vec<Entry>)> {
    let (where_sql, values) = filter.sql();
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM entries e WHERE {where_sql}"),
        params_from_iter(values.iter()),
        |r| r.get(0),
    )?;
    let mut page_values = values;
    page_values.push(rusqlite::types::Value::Integer(limit.clamp(1, MAX_PAGE_SIZE) as i64));
    page_values.push(rusqlite::types::Value::Integer(offset as i64));
    let mut stmt = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM entries e WHERE {where_sql} ORDER BY e.created_at DESC, e.id DESC LIMIT ? OFFSET ?"
    ))?;
    let mut entries: Vec<Entry> =
        stmt.query_map(params_from_iter(page_values.iter()), entry_from_row)?.collect::<rusqlite::Result<_>>()?;
    attach_tags(conn, &mut entries)?;
    Ok((total, entries))
}

pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM entries WHERE id = ?1", params![id])? > 0)
}

#[derive(Debug, Clone, Serialize)]
pub struct LabelCount {
    pub name: String,
    pub count: i64,
}

/// Tag frequencies, most used first; optionally only within one category.
pub fn tags(conn: &Connection, category: Option<&str>) -> Result<Vec<LabelCount>> {
    let category = category.and_then(normalize_label);
    let mut stmt = conn.prepare(
        "SELECT t.tag, COUNT(*) AS n FROM entry_tags t JOIN entries e ON e.id = t.entry_id \
         WHERE ?1 IS NULL OR e.category = ?1 GROUP BY t.tag ORDER BY n DESC, t.tag",
    )?;
    let rows = stmt.query_map(params![category], |r| Ok(LabelCount { name: r.get(0)?, count: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn categories(conn: &Connection) -> Result<Vec<LabelCount>> {
    let mut stmt = conn.prepare(
        "SELECT category, COUNT(*) AS n FROM entries GROUP BY category ORDER BY n DESC, category",
    )?;
    let rows = stmt.query_map([], |r| Ok(LabelCount { name: r.get(0)?, count: r.get(1)? }))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    Lexical,
    Dense,
    Hybrid,
}

impl std::str::FromStr for SearchMode {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "lexical" => Ok(Self::Lexical),
            "dense" => Ok(Self::Dense),
            "hybrid" => Ok(Self::Hybrid),
            other => bail!("mode must be lexical, dense or hybrid, not `{other}`"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub score: f64,
    pub snippet: String,
    #[serde(flatten)]
    pub entry: Entry,
}

/// An FTS5 query from free text: every word quoted, ORed. Quoting is what
/// keeps `codefather.dev` or `v0.13.0` from being parsed as FTS5 syntax
/// (issue 001); OR keeps a long question from matching nothing.
pub fn fts_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!words.is_empty()).then(|| words.join(" OR "))
}

/// Ranked entries for `query`. `embedding` is the query vector when the
/// caller could encode it; without one, dense and hybrid quietly become
/// lexical and the returned mode says so.
pub fn search(
    conn: &Connection,
    query: &str,
    embedding: Option<&[f32]>,
    mode: SearchMode,
    filter: &Filter,
    top_k: usize,
) -> Result<(SearchMode, Vec<Hit>)> {
    let top_k = top_k.clamp(1, MAX_TOP_K);
    let candidates = (top_k * 4).min(200);
    let effective = match (mode, embedding) {
        (SearchMode::Lexical, _) | (_, None) => SearchMode::Lexical,
        (m, Some(_)) => m,
    };
    let lexical = if effective == SearchMode::Dense { Vec::new() } else { lexical_ids(conn, query, filter, candidates)? };
    let dense = match (effective, embedding) {
        (SearchMode::Lexical, _) | (_, None) => Vec::new(),
        (_, Some(e)) => dense_ids(conn, e, filter, candidates)?,
    };
    let ranked: Vec<(i64, f64)> = match effective {
        SearchMode::Lexical => lexical,
        SearchMode::Dense => dense,
        SearchMode::Hybrid => rrf(&lexical, &dense),
    };
    let mut hits = Vec::new();
    for (id, score) in ranked.into_iter().take(top_k) {
        if let Some(entry) = get(conn, id)? {
            hits.push(Hit { score, snippet: snippet(&entry.body, query), entry });
        }
    }
    // What is no longer true still answers "what did we think", but never
    // ahead of what is current.
    hits.sort_by_key(|h| h.entry.flags.iter().any(|f| f.flag == "deprecated"));
    Ok((effective, hits))
}

fn lexical_ids(conn: &Connection, query: &str, filter: &Filter, k: usize) -> Result<Vec<(i64, f64)>> {
    let Some(fts) = fts_query(query) else { return Ok(Vec::new()) };
    let (where_sql, mut values) = filter.sql();
    values.insert(0, rusqlite::types::Value::Text(fts));
    values.push(rusqlite::types::Value::Integer(k as i64));
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id, -bm25(entries_fts) AS score FROM entries_fts JOIN entries e ON e.id = entries_fts.rowid \
         WHERE entries_fts MATCH ? AND {where_sql} ORDER BY score DESC LIMIT ?"
    ))?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn dense_ids(conn: &Connection, embedding: &[f32], filter: &Filter, k: usize) -> Result<Vec<(i64, f64)>> {
    let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut stmt = conn.prepare("SELECT rowid, distance FROM entries_vec WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance")?;
    let near: Vec<(i64, f64)> = stmt
        .query_map(params![bytes, k as i64], |r| Ok((r.get::<_, i64>(0)?, -r.get::<_, f64>(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if near.is_empty() {
        return Ok(near);
    }
    // vec0 cannot filter, so keep only ids the filter admits.
    let (where_sql, mut values) = filter.sql();
    values.extend(near.iter().map(|(id, _)| rusqlite::types::Value::Integer(*id)));
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id FROM entries e WHERE {where_sql} AND e.id IN ({})",
        vec!["?"; near.len()].join(",")
    ))?;
    let allowed: HashSet<i64> =
        stmt.query_map(params_from_iter(values.iter()), |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
    Ok(near.into_iter().filter(|(id, _)| allowed.contains(id)).collect())
}

fn rrf(a: &[(i64, f64)], b: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut scores: HashMap<i64, f64> = HashMap::new();
    for list in [a, b] {
        for (rank, (id, _)) in list.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut out: Vec<(i64, f64)> = scores.into_iter().collect();
    out.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal).then(y.0.cmp(&x.0)));
    out
}

/// Up to 200 characters around the first query word found, else the start.
fn snippet(body: &str, query: &str) -> String {
    let lower = body.to_lowercase();
    let start = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 2)
        .filter_map(|w| lower.find(&w.to_lowercase()))
        .min()
        .unwrap_or(0);
    // `find` on the lowercased copy gives a byte offset that may not exist in
    // the original when lowercasing changed lengths; fall back to a char count.
    let chars_before = lower[..start].chars().count();
    let from = chars_before.saturating_sub(SNIPPET_CHARS / 4);
    let total = body.chars().count();
    let text: String = body.chars().skip(from).take(SNIPPET_CHARS).collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}{text}{}", if from > 0 { "…" } else { "" }, if from + SNIPPET_CHARS < total { "…" } else { "" })
}

/// Entries that have no vector yet, oldest first.
pub fn missing_vectors(conn: &Connection, limit: usize) -> Result<Vec<(i64, String)>> {
    let mut stmt = conn.prepare(
        "SELECT e.id, e.body FROM entries e WHERE NOT EXISTS (SELECT 1 FROM entries_vec v WHERE v.rowid = e.id) \
         ORDER BY e.id LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

pub fn store_vector(conn: &mut Connection, id: i64, embedding: &[f32]) -> Result<()> {
    let bytes: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    // vec0 has no upsert, so the replace is a delete and an insert — in one
    // transaction, or two embedding passes over the same entry collide on the
    // rowid. The entry may also have been deleted in between, in which case
    // there is nothing to attach the vector to.
    let tx = write_tx(conn)?;
    if tx.query_row("SELECT 1 FROM entries WHERE id = ?1", params![id], |_| Ok(())).optional()?.is_none() {
        return Ok(());
    }
    tx.execute("DELETE FROM entries_vec WHERE rowid = ?1", params![id])?;
    tx.execute("INSERT INTO entries_vec(rowid, embedding) VALUES (?1, ?2)", params![id, bytes])?;
    tx.commit()?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GcReport {
    pub medium: i64,
    pub low: i64,
}

/// Drop entries past their salience's retention — except those with an open
/// work item or an `important` mark, which stay until it is closed or removed.
pub fn gc(conn: &Connection, now: i64, dry_run: bool) -> Result<GcReport> {
    let count = |salience: &str, ttl: i64| -> rusqlite::Result<i64> {
        conn.query_row(
            "SELECT COUNT(*) FROM entries WHERE salience = ?1 AND created_at < ?2 AND id NOT IN (SELECT entry_id FROM entry_flags WHERE closed_at IS NULL AND flag NOT IN ('idea', 'concept', 'comment', 'deprecated', 'verified'))",
            params![salience, now - ttl],
            |r| r.get(0),
        )
    };
    let report = GcReport { medium: count("medium", MEDIUM_TTL_SECS)?, low: count("low", LOW_TTL_SECS)? };
    if !dry_run && report.medium + report.low > 0 {
        conn.execute(
            "DELETE FROM entries WHERE ((salience = 'medium' AND created_at < ?1) OR (salience = 'low' AND created_at < ?2)) \
             AND id NOT IN (SELECT entry_id FROM entry_flags WHERE closed_at IS NULL AND flag NOT IN ('idea', 'concept', 'comment', 'deprecated', 'verified'))",
            params![now - MEDIUM_TTL_SECS, now - LOW_TTL_SECS],
        )?;
    }
    Ok(report)
}

#[derive(Debug, Clone, Serialize)]
pub struct JournalInfo {
    pub project: String,
    pub file: String,
    pub entries: i64,
    pub last_entry_at: Option<i64>,
}

/// Every journal the daemon holds. A file that cannot be read is skipped, not
/// fatal: one damaged journal must not hide the others.
pub fn list_journals(dir: &Path) -> Result<Vec<JournalInfo>> {
    let Ok(read) = std::fs::read_dir(dir) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("db") {
            continue;
        }
        let Ok(conn) = open_file(&path) else { continue };
        let project: Option<String> = conn
            .query_row("SELECT value FROM journal_meta WHERE key = 'project_path'", [], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        let Some(project) = project else { continue };
        let (entries, last): (i64, Option<i64>) = conn
            .query_row("SELECT COUNT(*), MAX(created_at) FROM entries", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap_or((0, None));
        out.push(JournalInfo {
            project,
            file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            entries,
            last_entry_at: last,
        });
    }
    out.sort_by(|a, b| b.last_entry_at.cmp(&a.last_entry_at).then(a.project.cmp(&b.project)));
    Ok(out)
}

// ---------------------------------------------------------------------------
// Sharing: one journal for several folders
// ---------------------------------------------------------------------------

/// Copy every entry of `src` into `dst` — tags, flags with their open/closed
/// state, and the links between them, renumbered — oldest first. An entry
/// `dst` already has (same text and category within the dedup window) is
/// merged, not duplicated. Returns (written, merged).
pub fn merge_into(dst: &mut Connection, src: &Connection) -> Result<(usize, usize)> {
    let ids: Vec<i64> = {
        let mut stmt = src.prepare("SELECT id FROM entries ORDER BY created_at, id")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut map: HashMap<i64, i64> = HashMap::new();
    let (mut written, mut merged) = (0, 0);
    for old in ids {
        let Some(e) = get(src, old)? else { continue };
        let entry = NewEntry {
            body: e.body.clone(),
            category: e.category.clone(),
            tags: e.tags.clone(),
            salience: e.salience.clone(),
            agent: e.agent.clone(),
            agent_id: e.agent_id.clone(),
            session_id: e.session_id.clone(),
            feature: e.feature.clone(),
            source_artifact: e.source_artifact.clone(),
            flags: e.flags.iter().map(|f| f.flag.clone()).collect(),
            proofs: e.proofs.clone(),
            // A reminder belongs to a session of this journal's machine.
            remind_every: None,
            remind_to: None,
            // Links are copied below by id; the text's numbers are src's.
            foreign: true,
            blocked_by: Vec::new(),
        };
        let (status, new) = add(dst, &entry, e.created_at)?;
        match status {
            AddStatus::Written => written += 1,
            AddStatus::Deduped => merged += 1,
        }
        map.insert(old, new);
        for f in e.flags.iter().filter(|f| !f.open) {
            let _ = close_with(
                dst,
                new,
                std::slice::from_ref(&f.flag),
                f.closed_by.as_deref(),
                f.resolution.as_deref(),
                f.closed_at.unwrap_or(e.created_at),
                CloseMode::Restore,
            );
        }
    }
    let links: Vec<(i64, i64, String)> = {
        let mut stmt = src.prepare("SELECT from_id, to_id, kind FROM entry_links")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (from, to, kind) in links {
        if let (Some(f), Some(t)) = (map.get(&from), map.get(&to))
            && f != t {
                dst.execute("INSERT OR IGNORE INTO entry_links(from_id, to_id, kind) VALUES (?1, ?2, ?3)", params![f, t, kind])?;
            }
    }
    // Mind maps point at entries by id: renumbered through the same map.
    crate::mindmap::merge(dst, src, &map).context("copy the mind maps")?;
    Ok((written, merged))
}

/// A complete copy of the journal `src` as the journal of `key`, ids and all —
/// what a folder takes with it when it stops sharing. Fails when `key` already
/// has a journal.
pub fn fork_to(src: &Connection, dir: &Path, key: &Path) -> Result<PathBuf> {
    let target = journal_path(dir, key);
    if target.exists() {
        bail!("{} already has a journal ({})", key.display(), target.display());
    }
    create_private_dir(dir)?;
    src.execute("VACUUM INTO ?1", params![target.to_string_lossy()])
        .with_context(|| format!("copy the journal to {}", target.display()))?;
    restrict(&target);
    let copy = open_file(&target)?;
    copy.execute("UPDATE journal_meta SET value = ?1 WHERE key = 'project_path'", params![key.to_string_lossy()])?;
    copy.execute(
        "INSERT OR REPLACE INTO journal_meta(key, value) VALUES ('forked_at', ?1)",
        params![now_secs().to_string()],
    )?;
    Ok(target)
}

/// An entry as another journal holds it: everything but what is derived here.
#[derive(Debug, Clone, Deserialize)]
pub struct ImportedEntry {
    pub id: i64,
    pub created_at: i64,
    pub category: String,
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_salience")]
    pub salience: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub feature: Option<String>,
    #[serde(default)]
    pub source_artifact: Option<String>,
    #[serde(default)]
    pub flags: Vec<ImportedFlag>,
    #[serde(default)]
    pub links: Vec<ImportedLink>,
    #[serde(default)]
    pub proofs: Vec<String>,
}

fn default_salience() -> String {
    "medium".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportedFlag {
    pub flag: String,
    pub set_at: i64,
    #[serde(default)]
    pub closed_at: Option<i64>,
    #[serde(default)]
    pub closed_by: Option<String>,
    #[serde(default)]
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportedLink {
    pub id: i64,
    pub kind: String,
    /// `out` is this entry's own link; the other direction comes with the other entry.
    #[serde(default)]
    pub dir: Option<String>,
}

/// Puts these entries in under the ids they have, so that what cites them still means them. An id
/// already holding the same text gets its flags, tags, links and proofs brought up to date; one
/// holding other text refuses the whole import, with nothing written. Returns (new, updated).
pub fn import_exact(conn: &mut Connection, entries: &[ImportedEntry]) -> Result<(usize, usize)> {
    let t = conn.transaction()?;
    let (mut new, mut updated) = (0, 0);
    for e in entries {
        let sha = hex(&Sha256::digest(e.body.trim().as_bytes()));
        let held: Option<String> = t.query_row("SELECT sha256 FROM entries WHERE id = ?1", params![e.id], |r| r.get(0)).optional()?;
        match held {
            Some(other) if other != sha => bail!("entry {} here says something else; move this journal aside first (--aside)", e.id),
            Some(_) => {
                t.execute("DELETE FROM entry_flags WHERE entry_id = ?1", params![e.id])?;
                t.execute("DELETE FROM entry_tags WHERE entry_id = ?1", params![e.id])?;
                t.execute("DELETE FROM entry_proofs WHERE entry_id = ?1", params![e.id])?;
                updated += 1;
            }
            None => {
                t.execute(
                    "INSERT INTO entries(id, created_at, category, body, sha256, salience, agent, agent_id, session_id, feature, source_artifact) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    params![e.id, e.created_at, e.category, e.body.trim(), sha, e.salience, e.agent, e.agent_id, e.session_id, e.feature, e.source_artifact],
                )?;
                new += 1;
            }
        }
        for tag in &e.tags {
            t.execute("INSERT OR IGNORE INTO entry_tags(entry_id, tag) VALUES (?1, ?2)", params![e.id, tag])?;
        }
        for f in &e.flags {
            t.execute(
                "INSERT OR REPLACE INTO entry_flags(entry_id, flag, set_at, closed_at, closed_by, resolution) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![e.id, f.flag, f.set_at, f.closed_at, f.closed_by, f.resolution],
            )?;
        }
        for proof in &e.proofs {
            t.execute("INSERT OR IGNORE INTO entry_proofs(entry_id, proof) VALUES (?1, ?2)", params![e.id, proof])?;
        }
    }
    // Links once every entry is in: they point both ways through the list.
    for e in entries {
        for link in e.links.iter().filter(|l| l.dir.as_deref() != Some("in")) {
            let known: Option<i64> = t.query_row("SELECT 1 FROM entries WHERE id = ?1", params![link.id], |r| r.get(0)).optional()?;
            if known.is_some() {
                t.execute("INSERT OR IGNORE INTO entry_links(from_id, to_id, kind) VALUES (?1, ?2, ?3)", params![e.id, link.id, link.kind])?;
            }
        }
    }
    t.commit()?;
    Ok((new, updated))
}

/// Move the journal of `key` out of the way — renamed, not deleted, and out of
/// what `list_journals` reads. `None` when it has none.
pub fn retire(dir: &Path, key: &Path, why: &str) -> Result<Option<PathBuf>> {
    let from = journal_path(dir, key);
    if !from.exists() {
        return Ok(None);
    }
    {
        let conn = open_file(&from)?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    }
    let to = from.with_extension(format!("{why}-{}", now_secs()));
    std::fs::rename(&from, &to).with_context(|| format!("move {} aside", from.display()))?;
    for ext in ["db-wal", "db-shm"] {
        let _ = std::fs::remove_file(from.with_extension(ext));
    }
    Ok(Some(to))
}

/// Entries flagged `ai-feedback` in one journal: `open` only, or all.
pub fn feedback(conn: &Connection, open_only: bool) -> Result<Vec<Entry>> {
    let filter = Filter { flags: vec!["ai-feedback".into()], open: open_only.then_some(true), ..Default::default() };
    Ok(list(conn, &filter, 0, MAX_PAGE_SIZE)?.1)
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn journal() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open(dir.path(), Path::new("/work/proj"), true).unwrap().unwrap();
        (dir, conn)
    }

    fn entry(body: &str, category: &str, tags: &[&str]) -> NewEntry {
        NewEntry {
            body: body.into(),
            category: category.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            salience: "medium".into(),
            ..Default::default()
        }
    }

    #[test]
    fn file_names_are_readable_unique_and_safe() {
        let a = journal_file_name(Path::new("/home/me/my project"));
        let b = journal_file_name(Path::new("/srv/my project"));
        assert!(a.starts_with("my-project-") && a.ends_with(".db"), "{a}");
        assert_ne!(a, b);
        assert!(journal_file_name(Path::new("/")).starts_with("root-"));
        assert!(!journal_file_name(Path::new("/x/../..")).contains('/'));
    }

    #[test]
    fn a_journal_refuses_to_serve_another_project() {
        let dir = tempfile::tempdir().unwrap();
        open(dir.path(), Path::new("/a"), true).unwrap();
        let path = journal_path(dir.path(), Path::new("/a"));
        std::fs::copy(&path, journal_path(dir.path(), Path::new("/b"))).unwrap();
        let err = open(dir.path(), Path::new("/b"), true).unwrap_err().to_string();
        assert!(err.contains("belongs to /a"), "{err}");
        assert!(open(dir.path(), Path::new("/c"), false).unwrap().is_none());
    }

    #[test]
    fn labels_normalize_one_way() {
        assert_eq!(normalize_label("  #Design Decision "), Some("design-decision".into()));
        assert_eq!(normalize_label("#"), None);
        assert_eq!(normalize_tags(&["Nginx", "#nginx", "", "docker"]), vec!["nginx", "docker"]);
    }

    #[test]
    fn add_validates_and_dedups() {
        let (_d, mut c) = journal();
        assert!(add(&mut c, &entry("x", "", &["a"]), 10).is_err());
        assert!(add(&mut c, &entry("x", "decision", &[]), 10).is_err());
        assert!(add(&mut c, &entry("  ", "decision", &["a"]), 10).is_err());
        let (s, id) = add(&mut c, &entry("use WAL", "Decision", &["sqlite"]), 10).unwrap();
        assert_eq!(s, AddStatus::Written);
        let (s, again) = add(&mut c, &entry("use WAL", "decision", &["locking"]), 20).unwrap();
        assert_eq!((s, again), (AddStatus::Deduped, id));
        assert_eq!(get(&c, id).unwrap().unwrap().tags, vec!["sqlite", "locking"]);
        // Outside the window it is a new entry.
        let (s, _) = add(&mut c, &entry("use WAL", "decision", &["sqlite"]), 20 + DEDUP_WINDOW_SECS + 11).unwrap();
        assert_eq!(s, AddStatus::Written);
    }

    #[test]
    fn categories_tags_list_and_filters() {
        let (_d, mut c) = journal();
        add(&mut c, &entry("chose tokio", "decision", &["async", "runtime"]), 1).unwrap();
        add(&mut c, &entry("found a race in the bus", "finding", &["async", "bus"]), 2).unwrap();
        add(&mut c, &entry("vec0 cannot filter", "finding", &["sqlite"]), 3).unwrap();

        let cats = categories(&c).unwrap();
        assert_eq!((cats[0].name.as_str(), cats[0].count), ("finding", 2));
        let t = tags(&c, None).unwrap();
        assert_eq!((t[0].name.as_str(), t[0].count), ("async", 2));
        let t = tags(&c, Some("decision")).unwrap();
        assert_eq!(t.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(), vec!["async", "runtime"]);

        let (total, page) = list(&c, &Filter::default(), 0, 2).unwrap();
        assert_eq!(total, 3);
        assert_eq!(page[0].body, "vec0 cannot filter");
        let f = Filter { tags: vec!["bus".into(), "sqlite".into()], ..Default::default() };
        assert_eq!(list(&c, &f, 0, 10).unwrap().0, 2);
        let f = Filter { category: Some("Finding".into()), since: Some(3), ..Default::default() };
        assert_eq!(list(&c, &f, 0, 10).unwrap().0, 1);
    }

    /// A burst of adds from several connections while others store vectors and
    /// search — what the daemon does after every add — must never answer
    /// "database is locked". With deferred transactions it did, at once.
    #[test]
    fn concurrent_writers_wait_instead_of_failing() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().to_path_buf();
        open(&d, Path::new("/w/p"), true).unwrap();
        let errors = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let mut threads = Vec::new();
        for t in 0..3 {
            let (d, errors) = (d.clone(), errors.clone());
            threads.push(std::thread::spawn(move || {
                for i in 0..40 {
                    let r = open(&d, Path::new("/w/p"), true).and_then(|c| {
                        add(&mut c.unwrap(), &entry(&format!("entry {t} {i}"), "update", &["x"]), now_secs())
                    });
                    if let Err(e) = r {
                        errors.lock().unwrap().push(format!("add: {e:#}"));
                    }
                }
            }));
        }
        for _ in 0..2 {
            let (d, errors) = (d.clone(), errors.clone());
            threads.push(std::thread::spawn(move || {
                for _ in 0..100 {
                    let r = (|| -> Result<()> {
                        let mut c = open(&d, Path::new("/w/p"), false)?.unwrap();
                        for (id, _) in missing_vectors(&c, 32)? {
                            store_vector(&mut c, id, &[0.1f32; 384])?;
                        }
                        let _ = search(&c, "entry", None, SearchMode::Lexical, &Filter::default(), 10)?;
                        Ok(())
                    })();
                    if let Err(e) = r {
                        errors.lock().unwrap().push(format!("vectors/search: {e:#}"));
                    }
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let errors = errors.lock().unwrap();
        assert!(errors.is_empty(), "{} errors, e.g. {:?}", errors.len(), errors.first());
        let c = open(&d, Path::new("/w/p"), false).unwrap().unwrap();
        assert_eq!(list(&c, &Filter::default(), 0, 1).unwrap().0, 120);
    }

    #[test]
    fn a_repository_journal_adopts_the_directory_journal_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = Path::new("/home/me/proj");
        let key = project_key(Some("github.com/me/proj"), path);
        assert_eq!(key, PathBuf::from("git:github.com/me/proj"));
        assert_eq!(project_key(Some("local:abc"), path), path);
        assert_eq!(project_key(None, path), path);

        let mut c = open(dir.path(), path, true).unwrap().unwrap();
        add(&mut c, &entry("chose WAL", "decision", &["sqlite"]), 1).unwrap();
        drop(c);
        assert!(adopt_path_journal(dir.path(), &key, path).unwrap());
        assert!(!journal_path(dir.path(), path).exists());
        let c = open(dir.path(), &key, false).unwrap().expect("adopted");
        assert_eq!(list(&c, &Filter::default(), 0, 10).unwrap().0, 1);
        drop(c);
        // A second checkout of the same repository does not adopt over it.
        let other = Path::new("/srv/proj");
        let mut o = open(dir.path(), other, true).unwrap().unwrap();
        add(&mut o, &entry("prod note", "finding", &["x"]), 2).unwrap();
        drop(o);
        assert!(!adopt_path_journal(dir.path(), &key, other).unwrap());
        assert_eq!(project_key(Some("github.com/me/proj"), other), key, "both checkouts share the key");
    }

    #[test]
    fn every_entry_carries_its_creation_time() {
        let (_d, mut c) = journal();
        add(&mut c, &entry("bumped rusqlite to 0.32", "update", &["deps"]), 1_761_300_300).unwrap();
        let (_, page) = list(&c, &Filter::default(), 0, 10).unwrap();
        assert_eq!((page[0].created_at, page[0].created.as_str()), (1_761_300_300, "2025-10-24T10:05:00Z"));
        let json = serde_json::to_value(&page[0]).unwrap();
        assert_eq!((json["created_at"].as_i64(), json["created"].as_str()), (Some(1_761_300_300), Some("2025-10-24T10:05:00Z")));
    }

    #[test]
    fn lexical_search_ranks_filters_and_survives_punctuation() {
        let (_d, mut c) = journal();
        add(&mut c, &entry("deploy to codefather.dev failed on v0.13.0", "finding", &["deploy"]), 1).unwrap();
        add(&mut c, &entry("deploy pipeline uses release.yml", "decision", &["ci"]), 2).unwrap();
        let (mode, hits) = search(&c, "codefather.dev v0.13.0", None, SearchMode::Hybrid, &Filter::default(), 5).unwrap();
        assert_eq!(mode, SearchMode::Lexical);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].snippet.contains("codefather"));
        let f = Filter { category: Some("decision".into()), ..Default::default() };
        let (_, hits) = search(&c, "deploy", None, SearchMode::Lexical, &f, 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.category, "decision");
        assert!(search(&c, "...", None, SearchMode::Lexical, &Filter::default(), 5).unwrap().1.is_empty());
    }

    #[test]
    fn dense_leg_uses_stored_vectors_and_respects_filters() {
        let (_d, mut c) = journal();
        let (_, a) = add(&mut c, &entry("alpha", "finding", &["x"]), 1).unwrap();
        let (_, b) = add(&mut c, &entry("beta", "decision", &["y"]), 2).unwrap();
        assert_eq!(missing_vectors(&c, 10).unwrap().len(), 2);
        let mut va = vec![0.0f32; 384];
        va[0] = 1.0;
        let mut vb = vec![0.0f32; 384];
        vb[1] = 1.0;
        store_vector(&mut c, a, &va).unwrap();
        store_vector(&mut c, b, &vb).unwrap();
        assert!(missing_vectors(&c, 10).unwrap().is_empty());
        let (mode, hits) = search(&c, "zzz", Some(&vb), SearchMode::Dense, &Filter::default(), 1).unwrap();
        assert_eq!(mode, SearchMode::Dense);
        assert_eq!(hits[0].entry.id, b);
        let f = Filter { category: Some("finding".into()), ..Default::default() };
        let (_, hits) = search(&c, "zzz", Some(&vb), SearchMode::Dense, &f, 5).unwrap();
        assert_eq!(hits.iter().map(|h| h.entry.id).collect::<Vec<_>>(), vec![a]);
        // Deleting the entry takes its vector with it.
        assert!(delete(&c, b).unwrap());
        let n: i64 = c.query_row("SELECT COUNT(*) FROM entries_vec", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn gc_honours_salience() {
        let (_d, mut c) = journal();
        let now = 1_000_000_000;
        for (s, age) in [("high", 10_000 * 86_400), ("medium", 400 * 86_400), ("low", 100 * 86_400), ("low", 10 * 86_400)] {
            let mut e = entry(&format!("{s} {age}"), "finding", &["t"]);
            e.salience = s.into();
            add(&mut c, &e, now - age).unwrap();
        }
        let r = gc(&c, now, true).unwrap();
        assert_eq!((r.medium, r.low), (1, 1));
        assert_eq!(list(&c, &Filter::default(), 0, 10).unwrap().0, 4);
        gc(&c, now, false).unwrap();
        assert_eq!(list(&c, &Filter::default(), 0, 10).unwrap().0, 2);
    }

    #[test]
    fn flags_open_close_reopen_and_filter() {
        let (_d, mut c) = journal();
        let mut todo = entry("run the benchmark", "", &["perf"]);
        todo.flags = vec!["todo".into(), "#Important".into()];
        let (_, t) = add(&mut c, &todo, 10).unwrap();
        let got = get(&c, t).unwrap().unwrap();
        assert_eq!(got.category, "todo", "the first flag names an entry without a category");
        assert_eq!(got.flags.iter().map(|f| (f.flag.as_str(), f.open)).collect::<Vec<_>>(), vec![("todo", true), ("important", true)]);
        let (_, q) = add(&mut c, &entry("why is the bus slow?", "finding", &["bus"]), 11).unwrap();
        assert_eq!(mark(&mut c, q, &["question".into(), "problem".into()], 12).unwrap(), vec!["question", "problem"]);
        assert!(mark(&mut c, q, &["question".into()], 13).unwrap().is_empty(), "already set");
        assert!(mark(&mut c, q, &["urgent".into()], 13).is_err());

        let open = |c: &Connection, flags: &[&str], state: Option<bool>| -> Vec<i64> {
            let f = Filter { flags: flags.iter().map(|s| s.to_string()).collect(), open: state, ..Default::default() };
            list(c, &f, 0, 10).unwrap().1.iter().map(|e| e.id).collect()
        };
        assert_eq!(open(&c, &["question", "todo"], Some(true)), vec![q, t]);

        assert!(close(&mut c, t, &["important".into()], None, None, 20).is_err(), "important is unmarked, not closed");
        assert_eq!(close(&mut c, q, &["question".into()], Some("mira"), Some("lock contention"), 20).unwrap(), vec!["question"]);
        assert!(close(&mut c, q, &["question".into()], None, None, 21).is_err(), "already closed");
        assert_eq!(open(&c, &["question"], Some(true)), Vec::<i64>::new());
        assert_eq!(open(&c, &["question"], Some(false)), vec![q]);
        let f = get(&c, q).unwrap().unwrap().flags;
        assert_eq!((f[0].flag.as_str(), f[0].open, f[0].closed_by.as_deref(), f[0].resolution.as_deref()), ("question", false, Some("mira"), Some("lock contention")));
        // Closing with nothing named closes every open work item.
        assert_eq!(close(&mut c, q, &[], None, None, 22).unwrap(), vec!["problem"]);
        assert!(close(&mut c, q, &[], None, None, 23).is_err());
        assert_eq!(reopen(&mut c, q, &["question".into()]).unwrap(), vec!["question"]);
        assert_eq!(open(&c, &["question"], Some(true)), vec![q]);
        assert_eq!(unmark(&mut c, t, &["important".into()]).unwrap(), vec!["important"]);

        let a = agenda(&c, 10).unwrap();
        assert_eq!(a.iter().map(|(f, n, _)| (f.as_str(), *n)).collect::<Vec<_>>(), vec![("todo", 1), ("question", 1)]);
    }

    #[test]
    fn retention_spares_what_is_still_open_or_important() {
        let (_d, mut c) = journal();
        let now = 1_000_000_000;
        let old = now - 200 * 86_400;
        let mut low = |body: &str, flags: &[&str]| {
            let mut e = entry(body, "note", &["t"]);
            e.salience = "low".into();
            e.flags = flags.iter().map(|f| f.to_string()).collect();
            add(&mut c, &e, old).unwrap().1
        };
        let plain = low("plain", &[]);
        let todo = low("open todo", &["todo"]);
        let done = low("closed todo", &["todo"]);
        let star = low("highlight", &["important"]);
        close_with(&mut c, done, &[], None, Some("dropped"), old + 1, CloseMode::Abandon).unwrap();
        assert_eq!(gc(&c, now, true).unwrap().low, 2);
        gc(&c, now, false).unwrap();
        assert!(get(&c, plain).unwrap().is_none() && get(&c, done).unwrap().is_none());
        assert!(get(&c, todo).unwrap().is_some() && get(&c, star).unwrap().is_some());
    }

    #[test]
    fn deprecated_knowledge_ranks_last_and_is_not_kept() {
        let (_d, mut c) = journal();
        let (_, old) = add(&mut c, &entry("journal writes retry on SQLITE_BUSY", "decision", &["sqlite"]), 1).unwrap();
        let (_, new) = add(&mut c, &entry("journal writes take BEGIN IMMEDIATE instead of SQLITE_BUSY retries", "decision", &["sqlite"]), 2).unwrap();
        let (_, hits) = search(&c, "SQLITE_BUSY journal writes retry", None, SearchMode::Lexical, &Filter::default(), 5).unwrap();
        assert_eq!(hits[0].entry.id, old, "before deprecation the closer match leads");
        mark(&mut c, old, &["deprecated".into()], 3).unwrap();
        let (_, hits) = search(&c, "SQLITE_BUSY journal writes retry", None, SearchMode::Lexical, &Filter::default(), 5).unwrap();
        assert_eq!(hits.iter().map(|h| h.entry.id).collect::<Vec<_>>(), vec![new, old]);
        assert!(close(&mut c, old, &["deprecated".into()], None, None, 4).unwrap_err().to_string().contains("unmark"));

        let mut rule = entry("never run gh auth switch", "constraint-note", &["gh"]);
        rule.flags = vec!["constraint".into()];
        rule.salience = "low".into();
        let (_, rule) = add(&mut c, &rule, 1).unwrap();
        c.execute("UPDATE entries SET salience = 'low'", []).unwrap();
        gc(&c, 1 + 400 * 86_400, false).unwrap();
        assert!(get(&c, old).unwrap().is_none(), "a deprecated entry is not kept past retention");
        assert!(get(&c, rule).unwrap().is_some(), "a constraint is");
        let a = agenda(&c, 10).unwrap();
        assert_eq!(a.iter().map(|(f, _, _)| f.as_str()).collect::<Vec<_>>(), vec!["constraint"]);
    }

    #[test]
    fn specifications_constraints_and_notes_combine_and_keep_as_they_should() {
        let (_d, mut c) = journal();
        let mut flagged = |body: &str, flags: &[&str]| {
            let mut e = entry(body, "", &["spec"]);
            e.flags = flags.iter().map(|f| f.to_string()).collect();
            e.salience = "low".into();
            add(&mut c, &e, 1).unwrap().1
        };
        // Any number of flags on one entry.
        let spec = flagged("a session writes only to its own private mind maps", &["specification", "constraint"]);
        let report = flagged("hook prints nothing after compaction on macOS", &["bugreport"]);
        let idea = flagged("draw journal branches as flowcharts", &["idea", "concept"]);
        let note = flagged("this reads well", &["comment"]);
        let e = get(&c, spec).unwrap().unwrap();
        assert_eq!(e.flags.iter().map(|f| f.flag.as_str()).collect::<Vec<_>>(), vec!["specification", "constraint"]);
        assert_eq!(e.category, "specification", "the first flag names an entry without a category");
        assert_eq!(get(&c, report).unwrap().unwrap().category, "bug", "a bug report is a bug");
        // A specification is a mark, removed with unmark; a bug report a work item, closed.
        assert!(close(&mut c, spec, &["specification".into()], None, None, 2).is_err());
        let a = agenda(&c, 10).unwrap();
        assert_eq!(
            a.iter().map(|(f, _, _)| f.as_str()).collect::<Vec<_>>(),
            vec!["bugreport", "specification", "constraint"],
            "ideas, concepts and comments are not open work"
        );
        // Past retention: the specification and the open report stay, the notes go.
        c.execute("UPDATE entries SET salience = 'low'", []).unwrap();
        gc(&c, 1 + 400 * 86_400, false).unwrap();
        assert!(get(&c, spec).unwrap().is_some());
        assert!(get(&c, report).unwrap().is_some());
        assert!(get(&c, idea).unwrap().is_none());
        assert!(get(&c, note).unwrap().is_none());
        assert!(normalize_flags(&["proof"]).is_err(), "no proof flag: --proof is the evidence option");
    }

    #[test]
    fn references_are_read_with_their_kind() {
        assert_eq!(parse_refs("Corrects journal#40 (plan)"), vec![(40, "replaces")]);
        assert_eq!(parse_refs("joins the journal#40/#41 plan"), vec![(40, "cites"), (41, "cites")]);
        assert_eq!(parse_refs("see journal#6, #7 and #9. Fixes journal#26"), vec![(6, "cites"), (7, "cites"), (9, "cites"), (26, "fixes")]);
        assert_eq!(parse_refs("Replaces JOURNAL#3; cites journal#4"), vec![(3, "replaces"), (4, "cites")]);
        assert!(parse_refs("issue #12, journal# and journal#x").is_empty());
        // The verb must be about this reference, not somewhere in the sentence.
        assert_eq!(parse_refs("(a 'replaces' edge also sets the deprecated flag on the target, journal#39 flags)"), vec![(39, "cites")]);
        assert_eq!(parse_refs("this replaces the old journal#5"), vec![(5, "replaces")]);
        assert_eq!(parse_refs("Implements journal#7. Verifies journal#8"), vec![(7, "implements"), (8, "verifies")]);
        assert_eq!(parse_refs("the unverified journal#9"), vec![(9, "cites")], "unverified is not verifies");
    }

    fn todo(c: &mut Connection, body: &str) -> i64 {
        let e = NewEntry { flags: vec!["todo".into()], ..entry(body, "todo", &["t"]) };
        add(c, &e, 1000).unwrap().1
    }

    fn proven(c: &mut Connection, id: i64) {
        let body = format!("Verifies journal#{id}: ran the whole protocol on this machine once and it printed what it must");
        let e = NewEntry { flags: vec!["verified".into()], ..entry(&body, "verification", &["t"]) };
        add(c, &e, 1000).unwrap();
    }

    #[test]
    fn a_blocked_entry_closes_only_after_its_blocker() {
        let (_d, mut c) = journal();
        let (first, second) = (todo(&mut c, "the cause"), todo(&mut c, "what rests on it"));
        assert!(block(&mut c, second, first).unwrap());
        assert_eq!(get(&c, second).unwrap().unwrap().blocked_by, [first]);
        proven(&mut c, second);
        let refused = close(&mut c, second, &[], None, None, 2000).unwrap_err().to_string();
        assert!(refused.contains("blocked by #") && refused.contains(&first.to_string()), "{refused}");
        proven(&mut c, first);
        close(&mut c, first, &[], None, None, 2000).unwrap();
        assert!(get(&c, second).unwrap().unwrap().blocked_by.is_empty(), "shows only what is still open");
        close(&mut c, second, &[], None, None, 2000).unwrap();
    }

    #[test]
    fn a_blocker_reopened_blocks_again_and_abandoned_lets_go() {
        let (_d, mut c) = journal();
        let (first, second) = (todo(&mut c, "first"), todo(&mut c, "second"));
        block(&mut c, second, first).unwrap();
        proven(&mut c, first);
        proven(&mut c, second);
        close(&mut c, first, &[], None, None, 2000).unwrap();
        reopen(&mut c, first, &[]).unwrap();
        assert!(close(&mut c, second, &[], None, None, 2000).is_err(), "open again, so it holds again");
        close_with(&mut c, first, &[], None, Some("not needed"), 2000, CloseMode::Abandon).unwrap();
        close(&mut c, second, &[], None, None, 2000).unwrap();
    }

    #[test]
    fn dropped_work_waits_for_nothing() {
        let (_d, mut c) = journal();
        let (first, second) = (todo(&mut c, "first"), todo(&mut c, "second"));
        block(&mut c, second, first).unwrap();
        close_with(&mut c, second, &[], None, Some("dropped"), 2000, CloseMode::Abandon).unwrap();
    }

    #[test]
    fn a_block_cannot_be_walked_round_by_closing_its_branch() {
        let (_d, mut c) = journal();
        let (root, other) = (todo(&mut c, "the work"), todo(&mut c, "what it waits for"));
        let body = format!("Implements journal#{root}: the change");
        let part = NewEntry { flags: vec!["problem".into()], ..entry(&body, "update", &["t"]) };
        let part = add(&mut c, &part, 1000).unwrap().1;
        block(&mut c, part, other).unwrap();
        let verified = NewEntry { flags: vec!["verified".into()], ..entry(&format!("Verifies journal#{part}: ran it all on this machine once and it printed what it must"), "verification", &["t"]) };
        add(&mut c, &verified, 1000).unwrap();
        let refused = close(&mut c, root, &[], None, None, 2000).unwrap_err().to_string();
        assert!(refused.contains("branch") && refused.contains(&part.to_string()), "{refused}");
    }

    #[test]
    fn what_could_never_close_is_refused_when_set() {
        let (_d, mut c) = journal();
        let (a, b, d) = (todo(&mut c, "a"), todo(&mut c, "b"), todo(&mut c, "d"));
        assert!(block(&mut c, a, a).is_err(), "itself");
        assert!(block(&mut c, a, 999).is_err(), "no such entry");
        assert!(block(&mut c, 999, a).is_err(), "no such entry");
        block(&mut c, a, b).unwrap();
        block(&mut c, b, d).unwrap();
        let cycle = block(&mut c, d, a).unwrap_err().to_string();
        assert!(cycle.contains("neither could ever close"), "{cycle}");
        let note = add(&mut c, &entry("a plain note", "finding", &["t"]), 1000).unwrap().1;
        assert!(block(&mut c, a, note).is_err(), "a note never closes");
        assert!(block(&mut c, note, a).is_err(), "a note has nothing to close");
        assert!(unblock(&c, a, b).unwrap());
        assert!(!unblock(&c, a, b).unwrap());
        assert!(get(&c, a).unwrap().unwrap().blocked_by.is_empty());
    }

    #[test]
    fn a_refused_entry_leaves_nothing_behind() {
        let (_d, mut c) = journal();
        let blocker = todo(&mut c, "blocker");
        for (flags, by) in [(vec!["todo".to_string()], 999), (vec![], blocker)] {
            let e = NewEntry { flags, blocked_by: vec![by], ..entry("should not stay", "todo", &["t"]) };
            assert!(add(&mut c, &e, 1000).is_err());
        }
        let stored: i64 = c.query_row("SELECT count(*) FROM entries", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 1);
        let e = NewEntry { flags: vec!["todo".to_string()], blocked_by: vec![blocker], ..entry("stays", "todo", &["t"]) };
        let id = add(&mut c, &e, 1000).unwrap().1;
        assert_eq!(get(&c, id).unwrap().unwrap().blocked_by, [blocker]);
    }

    #[test]
    fn a_block_survives_being_merged_into_another_journal() {
        let (_d, mut src) = journal();
        let (first, second) = (todo(&mut src, "the cause"), todo(&mut src, "what rests on it"));
        block(&mut src, second, first).unwrap();
        let (_d2, mut dst) = journal();
        todo(&mut dst, "already there, so the numbers differ");
        merge_into(&mut dst, &src).unwrap();
        let (_, hits) = list(&dst, &Filter::default(), 0, 10).unwrap();
        let moved = hits.iter().find(|e| e.body == "what rests on it").unwrap();
        let cause = hits.iter().find(|e| e.body == "the cause").unwrap();
        assert_eq!(moved.blocked_by, [cause.id]);
    }

    fn group(c: &Connection, name: &str) -> Vec<i64> {
        agenda(c, 10).unwrap().into_iter().find(|(flag, _, _)| flag == name).map_or(Vec::new(), |(_, _, entries)| entries.iter().map(|e| e.id).collect())
    }

    #[test]
    fn the_agenda_shows_what_can_be_taken_apart_from_what_waits() {
        let (_d, mut c) = journal();
        let (first, second, free) = (todo(&mut c, "first"), todo(&mut c, "second"), todo(&mut c, "free"));
        block(&mut c, second, first).unwrap();
        assert_eq!(group(&c, "todo"), [free, first]);
        assert_eq!(group(&c, "blocked"), [second]);
        let total = agenda(&c, 10).unwrap().into_iter().find(|(flag, _, _)| flag == "todo").unwrap().1;
        assert_eq!(total, 2, "the count says the same");
        proven(&mut c, first);
        close(&mut c, first, &[], None, None, 2000).unwrap();
        assert_eq!(group(&c, "todo"), [free, second], "taken up again");
        assert!(group(&c, "blocked").is_empty());
    }

    #[test]
    fn a_marked_entry_that_waits_still_shows_under_its_mark() {
        let (_d, mut c) = journal();
        let blocker = todo(&mut c, "blocker");
        let e = NewEntry { flags: vec!["todo".into(), "important".into()], blocked_by: vec![blocker], ..entry("both", "todo", &["t"]) };
        let id = add(&mut c, &e, 1000).unwrap().1;
        assert_eq!(group(&c, "important"), [id], "a mark is not work to take");
        assert_eq!(group(&c, "todo"), [blocker]);
    }

    #[test]
    fn a_close_says_what_it_set_free() {
        let (_d, mut c) = journal();
        let (first, other, one, both) = (todo(&mut c, "first"), todo(&mut c, "other"), todo(&mut c, "one"), todo(&mut c, "both"));
        block(&mut c, one, first).unwrap();
        block(&mut c, both, first).unwrap();
        block(&mut c, both, other).unwrap();
        proven(&mut c, first);
        close(&mut c, first, &[], None, None, 2000).unwrap();
        assert_eq!(released_by(&c, first).unwrap(), [one], "not what another still holds");
        proven(&mut c, other);
        close(&mut c, other, &[], None, None, 2000).unwrap();
        assert_eq!(released_by(&c, other).unwrap(), [both]);
        proven(&mut c, one);
        close(&mut c, one, &[], None, None, 2000).unwrap();
        assert_eq!(released_by(&c, first).unwrap(), [both], "what closed is not told again");
    }

    #[test]
    fn closing_a_todo_counts_what_waited_for_its_branch() {
        let (_d, mut c) = journal();
        let (root, waits) = (todo(&mut c, "the work"), todo(&mut c, "waits for a part of it"));
        let part = NewEntry { flags: vec!["problem".into()], ..entry(&format!("Implements journal#{root}: the change"), "update", &["t"]) };
        let part = add(&mut c, &part, 1000).unwrap().1;
        block(&mut c, waits, part).unwrap();
        let body = format!("Verifies journal#{part}: ran it all on this machine once and it printed what it must");
        add(&mut c, &NewEntry { flags: vec!["verified".into()], ..entry(&body, "verification", &["t"]) }, 1000).unwrap();
        close(&mut c, root, &[], None, None, 2000).unwrap();
        assert_eq!(released_by(&c, root).unwrap(), [waits]);
    }

    #[test]
    fn a_dropped_blocker_is_not_silent_until_the_entry_is_seen_to() {
        let (_d, mut c) = journal();
        let (dropped, waits, solved, after) = (todo(&mut c, "dropped"), todo(&mut c, "waits"), todo(&mut c, "solved"), todo(&mut c, "after"));
        block(&mut c, waits, dropped).unwrap();
        block(&mut c, after, solved).unwrap();
        close_with(&mut c, dropped, &[], None, Some("not worth it"), 2000, CloseMode::Abandon).unwrap();
        assert_eq!(released_by(&c, dropped).unwrap(), [waits]);
        assert_eq!(get(&c, waits).unwrap().unwrap().blocker_dropped, [dropped]);
        assert_eq!(group(&c, "blocker_dropped"), [waits]);
        assert!(group(&c, "todo").contains(&waits), "it can be taken: it waits for nothing now");

        proven(&mut c, solved);
        close(&mut c, solved, &[], None, None, 2000).unwrap();
        assert!(get(&c, after).unwrap().unwrap().blocker_dropped.is_empty(), "solved is not dropped");

        assert!(unblock(&c, waits, dropped).unwrap());
        assert!(group(&c, "blocker_dropped").is_empty(), "lifted by someone who read it");

        block(&mut c, after, waits).unwrap();
        reopen(&mut c, dropped, &[]).unwrap();
        block(&mut c, waits, dropped).unwrap();
        close_with(&mut c, dropped, &[], None, Some("again"), 2000, CloseMode::Abandon).unwrap();
        assert_eq!(group(&c, "blocker_dropped"), [waits]);
        close_with(&mut c, waits, &[], None, Some("no longer wanted"), 2000, CloseMode::Abandon).unwrap();
        assert!(!group(&c, "blocker_dropped").contains(&waits), "closed, so nothing is left to re-read");
    }

    #[test]
    fn dropping_any_work_item_says_it_was_dropped() {
        let (_d, mut c) = journal();
        let e = NewEntry { flags: vec!["question".into()], ..entry("a question", "question", &["t"]) };
        let (question, waits) = (add(&mut c, &e, 1000).unwrap().1, todo(&mut c, "waits"));
        block(&mut c, waits, question).unwrap();
        close_with(&mut c, question, &[], None, Some("moot"), 2000, CloseMode::Abandon).unwrap();
        assert_eq!(get(&c, question).unwrap().unwrap().flags[0].resolution.as_deref(), Some("abandoned: moot"));
        assert_eq!(get(&c, waits).unwrap().unwrap().blocker_dropped, [question]);
    }

    #[test]
    fn a_block_joins_no_branches_and_cannot_be_made_as_a_plain_link() {
        let (_d, mut c) = journal();
        let (a, b) = (todo(&mut c, "a"), todo(&mut c, "b"));
        block(&mut c, a, b).unwrap();
        proven(&mut c, b);
        close(&mut c, b, &[], None, None, 2000).unwrap();
        assert_eq!(get(&c, a).unwrap().unwrap().flags.iter().filter(|f| f.open).count(), 1, "closing the blocker closed nothing else");
        assert!(link(&mut c, a, b, "blocked_by").is_err(), "only by `block`, which refuses cycles");
    }

    #[test]
    fn links_come_from_the_text_and_both_ends_see_them() {
        let (_d, mut c) = journal();
        let (_, a) = add(&mut c, &entry("retry on SQLITE_BUSY", "decision", &["sqlite"]), 1).unwrap();
        let (_, b) = add(&mut c, &entry(&format!("Replaces journal#{a}: BEGIN IMMEDIATE. See journal#999"), "decision", &["sqlite"]), 2).unwrap();
        let got = get(&c, a).unwrap().unwrap();
        assert_eq!(got.links, vec![Link { kind: "replaces".into(), dir: "in", id: b }]);
        assert!(got.flags.is_empty(), "a replaces link never deprecates by itself");
        assert_eq!(get(&c, b).unwrap().unwrap().links, vec![Link { kind: "replaces".into(), dir: "out", id: a }], "#999 does not exist");

        let (_, n) = add(&mut c, &entry("unrelated note", "finding", &["x"]), 3).unwrap();
        assert!(link(&mut c, n, a, "cites").unwrap());
        assert!(!link(&mut c, n, a, "cites").unwrap());
        assert!(link(&mut c, n, n, "cites").is_err() && link(&mut c, n, a, "likes").is_err() && link(&mut c, n, 404, "cites").is_err());
        assert_eq!(unlink(&c, n, a, None).unwrap(), 1);

        // Imported text numbers another journal's entries: no links from it.
        let mut foreign = entry(&format!("Replaces journal#{a} over there"), "decision", &["x"]);
        foreign.foreign = true;
        let (_, f) = add(&mut c, &foreign, 4).unwrap();
        assert!(get(&c, f).unwrap().unwrap().links.is_empty());
        // Deleting an entry takes its links with it.
        assert!(delete(&c, b).unwrap());
        assert!(get(&c, a).unwrap().unwrap().links.is_empty());
    }

    #[test]
    fn an_older_journal_gets_its_links_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = open(dir.path(), Path::new("/w/old"), true).unwrap().unwrap();
        add(&mut c, &entry("first", "decision", &["t"]), 1).unwrap();
        add(&mut c, &entry("Fixes journal#1", "bug", &["t"]), 2).unwrap();
        // As if written before links existed.
        c.execute_batch("DELETE FROM entry_links; DELETE FROM journal_meta WHERE key = 'links_parsed';").unwrap();
        drop(c);
        let c = open(dir.path(), Path::new("/w/old"), false).unwrap().unwrap();
        assert_eq!(get(&c, 1).unwrap().unwrap().links, vec![Link { kind: "fixes".into(), dir: "in", id: 2 }]);
    }

    #[test]
    fn related_shows_links_meaning_and_source_with_flags() {
        let (_d, mut c) = journal();
        let v = |i: usize, j: usize, w: f32| {
            let mut v = vec![0.0f32; 384];
            v[i] = w;
            v[j] = (1.0 - w * w).sqrt();
            v
        };
        let mut src = entry("the bus drops frames", "finding", &["bus"]);
        src.source_artifact = Some("src/bus.rs".into());
        let (_, a) = add(&mut c, &src, 1).unwrap();
        let (_, near) = add(&mut c, &entry("frames lost under load", "finding", &["bus"]), 2).unwrap();
        let (_, far) = add(&mut c, &entry("telegram pairing", "decision", &["telegram"]), 3).unwrap();
        let mut same = entry("bus.rs rewritten", "update", &["bus"]);
        same.source_artifact = Some("src/bus.rs".into());
        let (_, s) = add(&mut c, &same, 4).unwrap();
        let (_, cites) = add(&mut c, &entry(&format!("see journal#{a}"), "finding", &["bus"]), 5).unwrap();
        store_vector(&mut c, a, &v(0, 1, 1.0)).unwrap();
        store_vector(&mut c, near, &v(0, 1, 0.9)).unwrap();
        store_vector(&mut c, far, &v(2, 3, 1.0)).unwrap();
        store_vector(&mut c, s, &v(4, 5, 1.0)).unwrap();
        store_vector(&mut c, cites, &v(0, 1, 0.95)).unwrap();
        mark(&mut c, near, &["deprecated".into()], 6).unwrap();

        let r = related(&c, a, 1).unwrap();
        assert_eq!(r.linked.iter().map(|(l, b)| (l.kind.as_str(), l.dir, b.id)).collect::<Vec<_>>(), vec![("cites", "in", cites)]);
        assert_eq!(r.similar.len(), 1);
        let (score, b) = &r.similar[0];
        assert_eq!((b.id, b.flags.clone()), (near, vec!["deprecated".to_string()]), "the linked entry is not repeated; flags travel");
        assert!((score - 0.9).abs() < 1e-3, "{score}");
        assert_eq!(r.same_source.iter().map(|b| b.id).collect::<Vec<_>>(), vec![s]);
        // Not embedded yet: no neighbours by meaning, the rest still there.
        let (_, fresh) = add(&mut c, &entry(&format!("Fixes journal#{a}"), "bug", &["bus"]), 7).unwrap();
        let r = related(&c, fresh, 3).unwrap();
        assert!(r.similar.is_empty());
        assert_eq!(r.linked[0].0, Link { kind: "fixes".into(), dir: "out", id: a });
    }

    #[test]
    fn a_todo_branch_closes_only_on_evidence_and_closes_whole() {
        let (_d, mut c) = journal();
        let mut todo = entry("make the pool clear a worker's context", "", &["pool"]);
        todo.flags = vec!["todo".into()];
        let (_, t) = add(&mut c, &todo, 1).unwrap();
        let mut imp = entry(&format!("Implements journal#{t}: POST /pool/<pool>/workers/<id>/clear"), "update", &["pool"]);
        imp.flags = vec!["unverified".into()];
        let (_, i) = add(&mut c, &imp, 2).unwrap();
        let (_, note) = add(&mut c, &entry(&format!("a side note on journal#{i}"), "finding", &["pool"]), 3).unwrap();
        assert_eq!(get(&c, i).unwrap().unwrap().links, vec![
            Link { kind: "cites".into(), dir: "in", id: note },
            Link { kind: "implements".into(), dir: "out", id: t },
        ]);

        // No verified entry yet: the todo refuses, and says how to proceed.
        let err = close(&mut c, t, &[], Some("w"), None, 4).unwrap_err().to_string();
        assert!(err.contains("--verified") && err.contains("--abandon"), "{err}");
        let b = branch(&c, i).unwrap().expect("any entry of the branch finds its todo");
        assert_eq!((b.root, b.state, b.verified), (t, "open", None));
        // A note that only cites the implementation is linked, not a member.
        assert_eq!(b.entries.iter().map(|e| e.entry.id).collect::<Vec<_>>(), vec![t, i]);
        assert!(branch(&c, note).unwrap().is_none());
        assert_eq!(b.entries[1].via.as_deref(), Some("implements"));

        let mut proof = entry(&format!("Verifies journal#{i}: e2e test passed, live clear in 2.6 s"), "verification", &["pool"]);
        proof.flags = vec!["verified".into()];
        let (_, v) = add(&mut c, &proof, 5).unwrap();
        assert_eq!(branch(&c, t).unwrap().unwrap().state, "verified");

        let closed = close(&mut c, t, &[], Some("w"), None, 6).unwrap();
        assert_eq!(closed, vec!["todo".to_string(), format!("#{i} unverified")]);
        let f = &get(&c, i).unwrap().unwrap().flags[0];
        assert_eq!((f.open, f.resolution.clone()), (false, Some(format!("verified: journal#{v}"))));
        assert_eq!(branch(&c, v).unwrap().unwrap().state, "closed");

        // Dropped work closes with a reason instead of evidence.
        let mut other = entry("port the pool to Windows", "", &["pool"]);
        other.flags = vec!["todo".into()];
        let (_, o) = add(&mut c, &other, 7).unwrap();
        assert!(close_with(&mut c, o, &[], None, None, 8, CloseMode::Abandon).is_err(), "a reason is required");
        close_with(&mut c, o, &[], None, Some("not needed"), 8, CloseMode::Abandon).unwrap();
        assert_eq!(get(&c, o).unwrap().unwrap().flags[0].resolution.as_deref(), Some("abandoned: not needed"));
        // An entry outside any todo's branch has none.
        let (_, lone) = add(&mut c, &entry("lone", "finding", &["x"]), 9).unwrap();
        assert!(branch(&c, lone).unwrap().is_none());
    }

    #[test]
    fn unverified_is_lifted_only_by_separate_evidence() {
        let (_d, mut c) = journal();
        let mut hyp = entry("the bus stalls because of WAL checkpoints", "finding", &["bus"]);
        hyp.flags = vec!["unverified".into()];
        let (_, h) = add(&mut c, &hyp, 1).unwrap();
        let open = |c: &Connection| get(c, h).unwrap().unwrap().flags.iter().any(|f| f.flag == "unverified" && f.open);

        // No evidence: neither close nor unmark lifts it.
        let err = close(&mut c, h, &[], None, Some("checked"), 2).unwrap_err().to_string();
        assert!(err.contains("no entry verifies it") && err.contains(&format!("Verifies journal#{h}")), "{err}");
        assert!(unmark(&mut c, h, &["unverified".into()]).is_err());
        assert!(open(&c));

        // A verifying entry that says too little does not count.
        let (_, thin) = add(&mut c, &entry(&format!("Verifies journal#{h}: ok"), "verification", &["bus"]), 3).unwrap();
        let err = close(&mut c, h, &[], None, None, 4).unwrap_err().to_string();
        assert!(err.contains(&format!("#{thin} says too little")), "{err}");

        // Evidence-first: a description alone is not enough, a proof is.
        set_evidence_first(&c, true).unwrap();
        let said = format!("Verifies journal#{h}: ran the load test 3 times on the 4060 box, checkpoint stalls 4-6 s each time");
        let (_, v) = add(&mut c, &entry(&said, "verification", &["bus"]), 5).unwrap();
        let err = close(&mut c, h, &[], None, None, 6).unwrap_err().to_string();
        assert!(err.contains("evidence-first") && err.contains("--proof"), "{err}");
        let mut proved = entry(&format!("{said}; trace attached"), "verification", &["bus"]);
        proved.proofs = vec!["docs/qa/bus-trace.png".into(), " ".into()];
        let (_, p) = add(&mut c, &proved, 7).unwrap();
        assert_eq!(get(&c, p).unwrap().unwrap().proofs, vec!["docs/qa/bus-trace.png"]);
        assert_eq!(close(&mut c, h, &[], Some("w"), None, 8).unwrap(), vec!["unverified"]);
        let f = &get(&c, h).unwrap().unwrap().flags[0];
        assert_eq!(f.resolution, Some(format!("verified: journal#{p}")));

        // Without evidence-first the description suffices; unmark is gated too.
        set_evidence_first(&c, false).unwrap();
        assert!(!evidence_first(&c).unwrap());
        let mut hyp2 = entry("the encoder download has no IPv4 fallback", "finding", &["encoder"]);
        hyp2.flags = vec!["unverified".into()];
        let (_, h2) = add(&mut c, &hyp2, 9).unwrap();
        let _ = v;
        add(&mut c, &entry(&format!("Verifies journal#{h2}: forced IPv6-only DNS twice, download hung both times"), "verification", &["encoder"]), 10).unwrap();
        assert_eq!(unmark(&mut c, h2, &["unverified".into()]).unwrap(), vec!["unverified"]);

        // Dropping the check is allowed, with a reason, and says so.
        let mut hyp3 = entry("maybe the ARM build is slower", "finding", &["build"]);
        hyp3.flags = vec!["unverified".into()];
        let (_, h3) = add(&mut c, &hyp3, 11).unwrap();
        assert!(close_with(&mut c, h3, &[], None, None, 12, CloseMode::Abandon).is_err());
        close_with(&mut c, h3, &[], None, Some("no ARM target"), 12, CloseMode::Abandon).unwrap();
        assert_eq!(get(&c, h3).unwrap().unwrap().flags[0].resolution.as_deref(), Some("abandoned: no ARM target"));
    }

    #[test]
    fn a_reminder_comes_back_to_its_author_until_closed() {
        let (_d, mut c) = journal();
        let me = Some(("mira".to_string(), Some("box".to_string())));
        // Without an interval a reminder is a plain work item.
        let mut plain = entry("look at the flaky test", "", &["ci"]);
        plain.flags = vec!["reminder".into()];
        plain.remind_to = me.clone();
        let (_, p) = add(&mut c, &plain, 0).unwrap();
        assert!(get(&c, p).unwrap().unwrap().remind.is_none());

        let mut r = entry("benchmark the bus", "todo", &["bus"]);
        r.remind_every = Some(30);
        r.remind_to = me.clone();
        assert!(add(&mut c, &r, 0).unwrap_err().to_string().contains("at least"));
        r.remind_every = Some(600);
        r.remind_to = None;
        assert!(add(&mut c, &r, 0).unwrap_err().to_string().contains("needs a session"));
        r.remind_to = me;
        let (_, id) = add(&mut c, &r, 1000).unwrap();
        let got = get(&c, id).unwrap().unwrap();
        assert!(got.flags.iter().any(|f| f.flag == "reminder" && f.open), "--every sets --reminder");
        assert_eq!(got.remind, Some(Reminder { every_secs: 600, next_at: 1600, nick: "mira".into() }));

        assert!(due_reminders(&c, 1599).unwrap().is_empty());
        let due = due_reminders(&c, 1600).unwrap();
        assert_eq!(due, vec![DueReminder { id, body: "benchmark the bus".into(), every_secs: 600, nick: "mira".into(), host: Some("box".into()) }]);
        // Delivered late (the author was away): the next one is an interval after delivery.
        reminder_sent(&c, id, 5000).unwrap();
        assert!(due_reminders(&c, 5599).unwrap().is_empty());
        assert_eq!(due_reminders(&c, 5600).unwrap().len(), 1);

        // Closed: silent. Reopened: back.
        close(&mut c, id, &["reminder".into()], Some("mira"), Some("done"), 5700).unwrap();
        assert!(due_reminders(&c, 99_999).unwrap().is_empty());
        reopen(&mut c, id, &["reminder".into()]).unwrap();
        assert_eq!(due_reminders(&c, 99_999).unwrap().len(), 1);
    }

    #[test]
    fn branches_link_into_a_graph_without_swallowing_each_other() {
        let (_d, mut c) = journal();
        let todo = |c: &mut Connection, body: &str, at| {
            let mut e = entry(body, "", &["t"]);
            e.flags = vec!["todo".into()];
            add(c, &e, at).unwrap().1
        };
        let flagged = |c: &mut Connection, body: &str, flag: &str, at| {
            let mut e = entry(body, "update", &["t"]);
            e.flags = vec![flag.into()];
            add(c, &e, at).unwrap().1
        };
        let a = todo(&mut c, "pinned dispatch", 1);
        let ai = flagged(&mut c, &format!("Implements journal#{a}"), "unverified", 2);
        let b = todo(&mut c, "clear a worker", 3);
        // B's implementation cites A's: a graph edge, not membership.
        let bi = flagged(&mut c, &format!("Implements journal#{b}, building on journal#{ai}"), "unverified", 4);
        // And a fix in B's branch for something in A's: still B's (its todo is nearer).
        let bf = flagged(&mut c, &format!("Implements journal#{b}. Fixes journal#{ai}"), "unverified", 5);
        flagged(&mut c, &format!("Verifies journal#{ai}: the pinned-dispatch e2e test passed three runs in a row"), "verified", 6);

        let ba = branch(&c, a).unwrap().unwrap();
        assert_eq!(ba.entries.iter().map(|e| e.entry.id).collect::<Vec<_>>(), vec![a, ai, 6]);
        assert_eq!(ba.linked_branches.iter().map(|l| l.root).collect::<Vec<_>>(), vec![b]);
        let bb = branch(&c, bi).unwrap().unwrap();
        assert_eq!((bb.root, bb.entries.len()), (b, 3), "{:?}", bb.entries.iter().map(|e| e.entry.id).collect::<Vec<_>>());
        assert!(bb.entries.iter().any(|e| e.entry.id == bf));
        assert_eq!(bb.linked_branches.iter().map(|l| l.root).collect::<Vec<_>>(), vec![a]);

        // Closing A closes A's work only.
        close(&mut c, a, &[], None, None, 7).unwrap();
        assert!(get(&c, bi).unwrap().unwrap().flags[0].open, "B's work stays open");
        assert!(get(&c, bf).unwrap().unwrap().flags[0].open);
        assert!(!get(&c, ai).unwrap().unwrap().flags[0].open);
        assert!(close(&mut c, b, &[], None, None, 8).is_err(), "B has no evidence of its own");
    }

    #[test]
    fn a_journal_merges_with_links_and_forks_with_its_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path(), Path::new("/w/a"), true).unwrap().unwrap();
        add(&mut a, &entry("shared knowledge", "finding", &["x"]), 1).unwrap();
        let mut b = open(dir.path(), Path::new("/w/b"), true).unwrap().unwrap();
        let mut t = entry("b's todo", "", &["y"]);
        t.flags = vec!["todo".into()];
        let (_, bt) = add(&mut b, &t, 2).unwrap();
        let mut done = entry("b's question", "", &["y"]);
        done.flags = vec!["question".into()];
        let (_, bq) = add(&mut b, &done, 3).unwrap();
        close(&mut b, bq, &[], Some("w"), Some("yes"), 4).unwrap();
        add(&mut b, &entry(&format!("Implements journal#{bt}"), "update", &["y"]), 5).unwrap();
        add(&mut b, &entry("shared knowledge", "finding", &["x"]), 6).unwrap();

        assert_eq!(merge_into(&mut a, &b).unwrap(), (3, 1));
        let (_, page) = list(&a, &Filter::default(), 0, 10).unwrap();
        let imp = page.iter().find(|e| e.body.starts_with("Implements")).unwrap();
        let todo = page.iter().find(|e| e.body == "b's todo").unwrap();
        assert_eq!(imp.links, vec![Link { kind: "implements".into(), dir: "out", id: todo.id }], "links renumbered");
        let q = page.iter().find(|e| e.body == "b's question").unwrap();
        assert_eq!((q.flags[0].open, q.flags[0].resolution.as_deref()), (false, Some("yes")));

        drop(b);
        let moved = retire(dir.path(), Path::new("/w/b"), "merged").unwrap().unwrap();
        assert!(moved.exists() && !journal_path(dir.path(), Path::new("/w/b")).exists());
        assert_eq!(list_journals(dir.path()).unwrap().len(), 1, "a retired journal is not listed");

        let fork = fork_to(&a, dir.path(), Path::new("/w/b")).unwrap();
        assert!(fork.exists());
        let f = open(dir.path(), Path::new("/w/b"), false).unwrap().expect("the fork is b's journal now");
        assert_eq!(get(&f, imp.id).unwrap().unwrap().links, imp.links, "same ids, same links");
        assert!(fork_to(&a, dir.path(), Path::new("/w/b")).is_err());
    }

    #[test]
    fn journals_are_enumerable() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = open(dir.path(), Path::new("/p/a"), true).unwrap().unwrap();
        add(&mut a, &entry("one", "finding", &["t"]), 50).unwrap();
        open(dir.path(), Path::new("/p/b"), true).unwrap();
        let all = list_journals(dir.path()).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!((all[0].project.as_str(), all[0].entries), ("/p/a", 1));
    }
}
