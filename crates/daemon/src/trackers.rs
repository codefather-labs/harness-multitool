//! Journal todos as issues in a tracker (ADR 0028, CON-139 to CON-143, journal#939). The daemon,
//! which owns the journals, queues what concerns a tracker as journal writes happen, and a worker
//! sends it in order, retrying, without a write ever waiting on a tracker. Per project (CON-142):
//! `<data dir>/journal-trackers/<project>/` holds `settings.json`, `issues.json` (a todo's issue)
//! and `queue.jsonl`. GitHub is reached through the unmodified gh CLI with GH_CONFIG_DIR always
//! named (CON-141); GitLab, Asana and ClickUp answer that they are not implemented yet (CON-143).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use harness_api::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How often the worker looks at the queues, and the least wait after a failure.
pub const TICK: Duration = Duration::from_secs(2);
/// The longest wait between two tries after failures.
pub const BACKOFF_MAX: Duration = Duration::from_secs(300);
/// The trackers there are, in the order the settings offer them.
pub const TRACKERS: [&str; 4] = ["github", "gitlab", "asana", "clickup"];

/// A project's tracker settings (CON-142): the daemon's, never in the repository.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// 'Send journal todos to issues'. Off until the user turns it on.
    #[serde(default)]
    pub enabled: bool,
    /// One of TRACKERS; github when not said.
    #[serde(default)]
    pub tracker: Option<String>,
    /// The gh configuration folder to run gh with. When not given, the project's own, made by the
    /// harness; it may name one the user already has, such as one per account.
    #[serde(default)]
    pub gh_config_dir: Option<PathBuf>,
    /// The gh program; `gh` on the PATH when not given.
    #[serde(default)]
    pub gh_binary: Option<String>,
    /// Sending to a public repository was confirmed (CON-141).
    #[serde(default)]
    pub public_ok: bool,
}

/// What to do on a tracker, queued in order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    /// A todo was added: its issue is opened.
    Open { id: u64 },
    /// An entry was added or linked: it is a comment on the issue of each todo it concerns, given
    /// or found from its references and its branch.
    Comment { entry: u64, todo: Option<u64> },
    /// A todo was closed, or abandoned (`not_planned`).
    Close { id: u64, resolution: String, not_planned: bool },
    /// A todo was reopened.
    Reopen { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub root: String,
    #[serde(flatten)]
    pub action: Action,
}

/// Where a project's tracker files are.
pub fn folder(data_dir: &Path, key: &Path) -> PathBuf {
    let name: String = key.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    data_dir.join("journal-trackers").join(name)
}

pub fn read_settings(folder: &Path) -> Settings {
    std::fs::read_to_string(folder.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_settings(folder: &Path, settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    write_whole(&folder.join("settings.json"), &serde_json::to_string_pretty(settings).expect("settings serialise"))
}

/// The gh configuration folder a project's GitHub runs with.
pub fn gh_dir(folder: &Path, settings: &Settings) -> PathBuf {
    settings.gh_config_dir.clone().unwrap_or_else(|| folder.join("gh"))
}

fn write_whole(path: &Path, text: &str) -> std::io::Result<()> {
    let staging = path.with_extension("partial");
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, path)
}

/// Adds an item to a project's queue.
pub fn enqueue(folder: &Path, item: &Item) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(folder.join("queue.jsonl"))?;
    std::io::Write::write_all(&mut file, format!("{}\n", serde_json::to_string(item).expect("an item serialises")).as_bytes())
}

fn queue(folder: &Path) -> Vec<Item> {
    std::fs::read_to_string(folder.join("queue.jsonl")).unwrap_or_default().lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn set_queue(folder: &Path, items: &[Item]) -> std::io::Result<()> {
    let text: String = items.iter().map(|i| format!("{}\n", serde_json::to_string(i).expect("an item serialises"))).collect();
    write_whole(&folder.join("queue.jsonl"), &text)
}

pub fn issues(folder: &Path) -> BTreeMap<u64, u64> {
    std::fs::read_to_string(folder.join("issues.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn set_issues(folder: &Path, map: &BTreeMap<u64, u64>) -> std::io::Result<()> {
    write_whole(&folder.join("issues.json"), &serde_json::to_string_pretty(map).expect("a map serialises"))
}

/// What a journal write means for a tracker, or nothing (the hook's part, in the daemon).
pub fn action_of(call: &crate::memory::Call, result: &Value) -> Option<Action> {
    use crate::memory::Call;
    match call {
        Call::JournalAdd { .. } => {
            let id = result["id"].as_u64()?;
            let flags: Vec<&str> = result["flags"].as_array()?.iter().filter_map(Value::as_str).collect();
            if flags.iter().any(|f| *f == "todo" || *f == "bugreport") {
                Some(Action::Open { id })
            } else {
                Some(Action::Comment { entry: id, todo: None })
            }
        }
        Call::JournalClose { id, resolution } => Some(Action::Close { id: *id, resolution: resolution.clone(), not_planned: false }),
        Call::More { more } => match more {
            MemoryOp::JournalReopen { id, .. } => Some(Action::Reopen { id: *id }),
            MemoryOp::JournalAbandon { id, resolution } => Some(Action::Close { id: *id, resolution: resolution.clone(), not_planned: true }),
            MemoryOp::JournalLink { from, to, .. } => Some(Action::Comment { entry: *from, todo: Some(*to) }),
            _ => None,
        },
        _ => None,
    }
}

/// owner/repo of a GitHub remote, in any of its forms.
pub fn github_place(remote: &str) -> Option<String> {
    let rest = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = rest
        .strip_prefix("git@github.com:")
        .or_else(|| rest.strip_prefix("ssh://git@github.com/"))
        .or_else(|| rest.strip_prefix("https://github.com/"))
        .or_else(|| rest.strip_prefix("http://github.com/"))
        .or_else(|| rest.strip_prefix("github.com:"))?;
    let mut parts = path.split('/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    (!owner.is_empty() && !repo.is_empty() && parts.next().is_none()).then(|| format!("{owner}/{repo}"))
}

/// A tracker (CON-143). `place` is its own address of the project: owner/repo on GitHub.
pub trait IssueTracker {
    fn name(&self) -> &str;
    /// As whom it is connected.
    fn connected(&self) -> Result<String, String>;
    /// Whether the place takes issues from this account, and whether it is public.
    fn visibility(&self, place: &str) -> Result<String, String>;
    fn open(&self, place: &str, title: &str, body: &str) -> Result<u64, String>;
    fn comment(&self, place: &str, issue: u64, body: &str) -> Result<(), String>;
    fn close(&self, place: &str, issue: u64, not_planned: bool, note: &str) -> Result<(), String>;
    fn reopen(&self, place: &str, issue: u64) -> Result<(), String>;
}

/// GitHub through the unmodified gh CLI (CON-141): GH_CONFIG_DIR always named, no token read, no
/// `gh auth switch`.
pub struct GitHub {
    pub binary: String,
    pub config_dir: PathBuf,
}

impl GitHub {
    fn run(&self, args: &[&str], input: Option<&str>) -> Result<String, String> {
        let mut command = Command::new(&self.binary);
        command.args(args).env("GH_CONFIG_DIR", &self.config_dir).env("GH_PROMPT_DISABLED", "1").env_remove("GH_TOKEN").env_remove("GITHUB_TOKEN");
        command.stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| format!("could not run {}: {e}", self.binary))?;
        if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
            std::io::Write::write_all(&mut stdin, text.as_bytes()).map_err(|e| e.to_string())?;
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            let said = String::from_utf8_lossy(&out.stderr);
            Err(format!("gh {}: {}", args.first().copied().unwrap_or_default(), said.lines().last().unwrap_or("failed").trim()))
        }
    }
}

impl IssueTracker for GitHub {
    fn name(&self) -> &str {
        "github"
    }

    fn connected(&self) -> Result<String, String> {
        self.run(&["api", "user", "--jq", ".login"], None)
    }

    fn visibility(&self, place: &str) -> Result<String, String> {
        self.run(&["repo", "view", place, "--json", "visibility,viewerPermission", "--jq", r#".visibility + " " + .viewerPermission"#], None)
    }

    fn open(&self, place: &str, title: &str, body: &str) -> Result<u64, String> {
        let url = self.run(&["issue", "create", "--repo", place, "--title", title, "--body-file", "-"], Some(body))?;
        url.lines().last().and_then(|u| u.rsplit('/').next()).and_then(|n| n.trim().parse().ok()).ok_or_else(|| format!("gh issue create answered no issue: {url}"))
    }

    fn comment(&self, place: &str, issue: u64, body: &str) -> Result<(), String> {
        self.run(&["issue", "comment", &issue.to_string(), "--repo", place, "--body-file", "-"], Some(body)).map(|_| ())
    }

    fn close(&self, place: &str, issue: u64, not_planned: bool, note: &str) -> Result<(), String> {
        let reason = if not_planned { "not planned" } else { "completed" };
        self.run(&["issue", "close", &issue.to_string(), "--repo", place, "--reason", reason, "--comment", note], None).map(|_| ())
    }

    fn reopen(&self, place: &str, issue: u64) -> Result<(), String> {
        self.run(&["issue", "reopen", &issue.to_string(), "--repo", place], None).map(|_| ())
    }
}

/// A tracker planned and not built yet (CON-143): every call says so.
pub struct NotYet(pub &'static str);

impl IssueTracker for NotYet {
    fn name(&self) -> &str {
        self.0
    }
    fn connected(&self) -> Result<String, String> {
        Err(format!("{} is not implemented yet", self.0))
    }
    fn visibility(&self, _place: &str) -> Result<String, String> {
        self.connected()
    }
    fn open(&self, _place: &str, _title: &str, _body: &str) -> Result<u64, String> {
        self.connected().map(|_| 0)
    }
    fn comment(&self, _place: &str, _issue: u64, _body: &str) -> Result<(), String> {
        self.connected().map(|_| ())
    }
    fn close(&self, _place: &str, _issue: u64, _not_planned: bool, _note: &str) -> Result<(), String> {
        self.connected().map(|_| ())
    }
    fn reopen(&self, _place: &str, _issue: u64) -> Result<(), String> {
        self.connected().map(|_| ())
    }
}

/// The tracker a project's settings choose.
pub fn tracker(folder: &Path, settings: &Settings) -> Box<dyn IssueTracker> {
    match settings.tracker.as_deref().unwrap_or("github") {
        "gitlab" => Box::new(NotYet("gitlab")),
        "asana" => Box::new(NotYet("asana")),
        "clickup" => Box::new(NotYet("clickup")),
        _ => Box::new(GitHub { binary: settings.gh_binary.clone().unwrap_or_else(|| "gh".to_string()), config_dir: gh_dir(folder, settings) }),
    }
}

/// An issue's title: the entry's first line, cut to fit.
pub fn title_of(record: &JournalRecord) -> String {
    let first = record.body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("journal todo");
    let mut title: String = first.chars().take(120).collect();
    if first.chars().count() > 120 {
        title.push('…');
    }
    format!("journal#{}: {title}", record.id)
}

/// An issue's body or a comment: the entry whole, its flags, tags and id.
pub fn text_of(record: &JournalRecord) -> String {
    let flags = if record.flags.is_empty() { String::new() } else { format!(" · flags: {}", record.flags.join(", ")) };
    let tags = if record.tags.is_empty() { String::new() } else { format!(" · tags: {}", record.tags.join(", ")) };
    format!("**journal#{}** · {}{flags}{tags}\n\n{}", record.id, record.category, record.body)
}

/// The `journal#N` an entry's text names.
pub fn references(text: &str) -> Vec<u64> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("journal#") {
        rest = &rest[at + "journal#".len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(n) = digits.parse()
            && !out.contains(&n)
        {
            out.push(n);
        }
    }
    out
}

/// What the worker reads of a project's journal.
pub trait Journal {
    fn get(&self, id: u64) -> Option<JournalRecord>;
    /// The todo at the root of an entry's branch.
    fn branch_root(&self, id: u64) -> Option<u64>;
}

/// Sends one item. Ok(()) when it is done or has nothing to do; Err to try again later.
pub fn send(item: &Item, folder: &Path, place: &str, tracker: &dyn IssueTracker, journal: &dyn Journal) -> Result<(), String> {
    let mut map = issues(folder);
    match &item.action {
        Action::Open { id } => {
            if map.contains_key(id) {
                return Ok(());
            }
            let Some(record) = journal.get(*id) else { return Ok(()) };
            let issue = tracker.open(place, &title_of(&record), &text_of(&record))?;
            map.insert(*id, issue);
            set_issues(folder, &map).map_err(|e| e.to_string())
        }
        Action::Comment { entry, todo } => {
            let Some(record) = journal.get(*entry) else { return Ok(()) };
            let mut todos: Vec<u64> = todo.iter().copied().collect();
            if todo.is_none() {
                todos.extend(references(&record.body));
                todos.extend(journal.branch_root(*entry).filter(|root| root != entry));
            }
            todos.dedup();
            let mut done = Vec::new();
            for todo in todos {
                if let Some(issue) = map.get(&todo).copied().filter(|issue| !done.contains(issue)) {
                    tracker.comment(place, issue, &text_of(&record))?;
                    done.push(issue);
                }
            }
            Ok(())
        }
        Action::Close { id, resolution, not_planned } => match map.get(id) {
            Some(issue) => tracker.close(place, *issue, *not_planned, &format!("Closed in the journal: {resolution}")),
            None => Ok(()),
        },
        Action::Reopen { id } => match map.get(id) {
            Some(issue) => tracker.reopen(place, *issue),
            None => Ok(()),
        },
    }
}

/// Why a project's queue is not being sent, in words for the settings and the status, or None.
pub fn why_not(settings: &Settings, place: Option<&str>, tracker: &dyn IssueTracker) -> Option<String> {
    if !settings.enabled {
        return Some("off: 'Send journal todos to issues' is not turned on".to_string());
    }
    let Some(place) = place else { return Some("the project has no GitHub remote (git remote get-url origin)".to_string()) };
    if let Err(e) = tracker.connected() {
        return Some(format!("not connected: {e}"));
    }
    match tracker.visibility(place) {
        Err(e) => Some(format!("{place}: {e}")),
        Ok(v) if v.starts_with("PUBLIC") && !settings.public_ok => Some(format!("{place} is public: confirm with `multitool journal issues on --public` to send its todos there")),
        Ok(v) if v.split_whitespace().nth(1).is_some_and(|p| matches!(p, "READ" | "NONE" | "")) => Some(format!("{place}: this account cannot write issues there ({v})")),
        Ok(_) => None,
    }
}

/// The worker: every tick, each project's queue that has items is sent in order while the tracker
/// takes them; a failure waits, doubling up to BACKOFF_MAX, and is tried again. Runs until `stop`.
pub fn run(data_dir: PathBuf, journal_of: impl Fn(&str) -> Option<Box<dyn Journal>>, stop: impl Fn() -> bool) {
    let mut wait: BTreeMap<PathBuf, (Instant, Duration)> = BTreeMap::new();
    while !stop() {
        std::thread::sleep(TICK);
        let Ok(projects) = std::fs::read_dir(data_dir.join("journal-trackers")) else { continue };
        for project in projects.flatten() {
            let folder = project.path();
            let items = queue(&folder);
            if items.is_empty() || wait.get(&folder).is_some_and(|(at, _)| Instant::now() < *at) {
                continue;
            }
            let settings = read_settings(&folder);
            if !settings.enabled {
                // Off: what was queued goes, nothing is sent (CON-139).
                let _ = set_queue(&folder, &[]);
                continue;
            }
            let root = items[0].root.clone();
            let place = crate::git_origin(&root).as_deref().and_then(github_place);
            let tracker = tracker(&folder, &settings);
            let failed = match (why_not(&settings, place.as_deref(), tracker.as_ref()), journal_of(&root)) {
                (Some(why), _) => Some(why),
                (None, None) => Some("the project's journal cannot be read".to_string()),
                (None, Some(journal)) => {
                    let mut left = items.clone();
                    let mut failed = None;
                    while let Some(item) = left.first().cloned() {
                        match send(&item, &folder, place.as_deref().unwrap_or_default(), tracker.as_ref(), journal.as_ref()) {
                            Ok(()) => {
                                left.remove(0);
                                let _ = set_queue(&folder, &left);
                            }
                            Err(e) => {
                                failed = Some(e);
                                break;
                            }
                        }
                    }
                    failed
                }
            };
            match failed {
                None => {
                    wait.remove(&folder);
                }
                Some(why) => {
                    let next = wait.get(&folder).map_or(TICK, |(_, d)| (*d * 2).min(BACKOFF_MAX));
                    wait.insert(folder.clone(), (Instant::now() + next, next));
                    let _ = std::fs::write(folder.join("last-error.txt"), &why);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_remote_in_every_form_is_one_place() {
        for url in ["git@github.com:Foo/bar.git", "https://github.com/Foo/bar", "ssh://git@github.com/Foo/bar.git", "https://github.com/Foo/bar/"] {
            assert_eq!(github_place(url).as_deref(), Some("Foo/bar"), "{url}");
        }
        assert_eq!(github_place("git@gitlab.com:a/b.git"), None);
        assert_eq!(github_place("/local/path"), None);
    }

    #[test]
    fn references_and_titles() {
        assert_eq!(references("Implements journal#12, cites journal#7 and journal#12."), [12, 7]);
        let record = JournalRecord { id: 5, body: format!("  \n{}", "x".repeat(200)), ..Default::default() };
        let title = title_of(&record);
        assert!(title.starts_with("journal#5: ") && title.ends_with('…') && title.chars().count() < 140, "{title}");
    }
}
