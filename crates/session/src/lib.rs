//! Runs sessions on a native build (roadmap M2): the configuration selects the plugins (IF-0.3),
//! and each session's loop runs on its own thread, talking to the frontend over channels.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use harness_api::*;
use harness_core::{Engine, Parts, Registry, RunEnd, Selection, StopFlag};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod host_tools;
mod wire;

/// One `[[session.hooks]]`: a command run on a loop event (every event when it names none).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HookSetting {
    pub event: Option<String>,
    pub command: String,
    /// What its card shows; the command by default.
    pub name: Option<String>,
}

/// The `[session]` part of the user's configuration. Everything has a default; a provider's
/// settings and keys live under `[providers.<name>]` and `[auth.<name>]`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub model: String,
    pub max_tokens: u32,
    pub max_turns: Option<u32>,
    pub max_cost_usd: Option<f64>,
    /// Compact the conversation above this many tokens; 70% of the context window by default.
    pub compact_at_tokens: Option<u64>,
    pub tools: Vec<String>,
    /// Shell hooks, in the order they run.
    pub hooks: Vec<HookSetting>,
    /// `{"anthropic": {"base_url": ..., "effort": ...}, "local": {"kind": "openai", "base_url": ...}}`.
    pub providers: BTreeMap<String, Value>,
    /// `{"anthropic": {"env": "..."}, "local": {"none": true}}`.
    pub auth: BTreeMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            provider: "anthropic".into(),
            model: "claude-opus-5-5".into(),
            max_tokens: 64_000,
            max_turns: None,
            max_cost_usd: None,
            compact_at_tokens: None,
            tools: ["read", "edit", "shell", "search", "fetch"].into_iter().chain(harness_tools_memory::NAMES).map(String::from).collect(),
            hooks: Vec::new(),
            providers: BTreeMap::new(),
            auth: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// The context window to show for the chosen model.
    pub fn context_window(&self) -> u64 {
        self.providers.get(&self.provider).and_then(|p| p["context_window"].as_u64()).unwrap_or(if self.provider == "anthropic" { 1_000_000 } else { 128_000 })
    }

    fn selection(&self) -> Selection {
        Selection {
            auth: "api-key".into(),
            providers: vec![self.provider.clone()],
            router: "one-model".into(),
            tools: self.tools.clone(),
            policy: "default".into(),
            hooks: (0..self.hooks.len()).map(|i| format!("shell-{i}")).chain(std::iter::once("docs-mirror".to_string())).collect(),
            memory: "journal".into(),
            context: "journal-aware".into(),
            store: "files".into(),
            verifier: "protocol".into(),
        }
    }
}

/// `MULTITOOL_WIRE_LOG=<file>`: see everything that goes to and comes from a model.
fn wire_log_path() -> Option<PathBuf> {
    std::env::var_os("MULTITOOL_WIRE_LOG").filter(|p| !p.is_empty()).map(PathBuf::from)
}

/// The folder's `origin` URL, if it is a repository with one.
fn git_origin(root: &str) -> Option<String> {
    let out = std::process::Command::new("git").args(["-C", root, "config", "--get", "remote.origin.url"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|u| !u.is_empty())
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Everything shipped in this build, under its configuration names.
pub fn registry(settings: &Settings, root: &str, data_dir: &str, note: Option<String>) -> Registry {
    let mut r = Registry::new();
    let (root_a, root_b, root_c) = (root.to_string(), root.to_string(), root.to_string());
    let context_budget = settings.compact_at_tokens.unwrap_or(settings.context_window() * 7 / 10);
    let sessions_dir = sessions_dir(data_dir, root);
    let (root_d, data_d) = (root.to_string(), std::path::PathBuf::from(data_dir));
    r.auth("api-key", |cx| Ok(Rc::new(harness_auth_apikey::ApiKeyAuth::new(env, cx.settings))))
        .router("one-model", |cx| Ok(Rc::new(harness_router_default::OneModel::new(cx.settings)?)))
        .tool("read", |_| Ok(Rc::new(harness_tools_builtin::Read)))
        .tool("edit", |_| Ok(Rc::new(harness_tools_builtin::Edit)))
        .tool("shell", |_| Ok(Rc::new(harness_tools_builtin::Shell)))
        .tool("search", |_| Ok(Rc::new(harness_tools_builtin::Search)))
        .tool("fetch", |_| Ok(Rc::new(harness_tools_builtin::Fetch)))
        .policy("default", move |_| Ok(Rc::new(harness_policy_default::DefaultPolicy::new(&root_a))))
        .memory("none", |_| Ok(Rc::new(harness_context_default::NoMemory)))
        .memory("journal", move |_| {
            // CON-20: the daemon owns the store. With none running (a debug run, a test) the
            // session opens it itself; SQLite in WAL mode keeps the two safe together.
            if let Some(client) = harness_daemon::client::Client::find(&data_d) {
                return Ok(Rc::new(harness_daemon::memory::DaemonMemory::new(client, data_d.clone(), &root_d, "multitool")));
            }
            let key = harness_memory_journal::project::key_for(git_origin(&root_d).as_deref(), std::path::Path::new(&root_d));
            Ok(Rc::new(harness_memory_journal::plugin::JournalMemory::open(&data_d.join("journals"), &key, "multitool")?))
        })
        .context("journal-aware", move |_| Ok(Rc::new(harness_context_default::JournalAware { root: root_b.clone(), note: note.clone(), budget_tokens: context_budget })))
        .store("files", move |cx| Ok(Rc::new(harness_session_files::FileSessions::new(cx.platform.fs.clone(), &sessions_dir))))
        .verifier("protocol", move |cx| Ok(Rc::new(harness_verifier_protocol::ProtocolRunner::new(cx.platform.process.clone(), &root_c))));
    for name in harness_tools_memory::NAMES {
        r.tool(name, move |cx| {
            let memory = cx.memory.clone().ok_or("the tool was built before Memory")?;
            let tool = harness_tools_memory::tool(memory, name).ok_or("no such memory tool")?;
            Ok(Rc::new(tool))
        });
    }
    let root_m = root.to_string();
    r.hook("docs-mirror", move |cx| {
        let memory = cx.memory.clone().ok_or("the docs mirror needs memory")?;
        Ok(Rc::new(harness_docs_mirror::DocsMirrorHook::new(cx.platform.fs.clone(), memory, &root_m)))
    });
    for (i, hook) in settings.hooks.iter().enumerate() {
        let (hook, root) = (hook.clone(), root.to_string());
        r.hook(&format!("shell-{i}"), move |cx| {
            let event = match &hook.event {
                Some(name) => Some(harness_hooks_shell::point_kind(name).ok_or_else(|| format!("no loop event named {name}"))?),
                None => None,
            };
            let name = hook.name.clone().unwrap_or_else(|| hook.command.clone());
            Ok(Rc::new(harness_hooks_shell::ShellHook::new(&name, event, &hook.command, &root, cx.platform.process.clone())))
        });
    }
    for name in std::iter::once(settings.provider.as_str()).chain(settings.providers.keys().map(String::as_str)) {
        let kind = settings.providers.get(name).and_then(|p| p["kind"].as_str()).map(str::to_string).unwrap_or_else(|| if name == "anthropic" { "anthropic".to_string() } else { "openai".to_string() });
        let provider_name = name.to_string();
        r.provider(name, move |cx| {
            let auth = cx.auth.clone().ok_or("the provider was built before Auth")?;
            let provider: Rc<dyn Provider> = if kind == "anthropic" {
                Rc::new(harness_provider_anthropic::Anthropic::new(auth, cx.platform.net.clone(), cx.platform.clock.clone(), cx.settings))
            } else {
                Rc::new(harness_provider_openai::OpenAiCompatible::new(&provider_name, auth, cx.platform.net.clone(), cx.settings))
            };
            Ok(provider)
        });
        if let Some(value) = settings.providers.get(name) {
            r.settings("provider", name, value.clone());
        }
    }
    r.settings("auth", "api-key", json!(settings.auth));
    r.settings(
        "router",
        "one-model",
        json!({"provider": settings.provider, "model": settings.model, "max_tokens": settings.max_tokens, "max_turns": settings.max_turns, "max_cost_usd": settings.max_cost_usd}),
    );
    r
}

/// Sessions are kept per project folder.
fn sessions_dir(data_dir: &str, root: &str) -> String {
    let slug: String = root.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    format!("{}/sessions/{}", data_dir.trim_end_matches('/'), slug.trim_matches('-'))
}

/// What the startup page shows of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    /// The first thing the user said.
    pub title: String,
    pub model: String,
    pub last_ms: u64,
}

/// A stored session's entries, cut off at a damaged last line (a crash can leave one).
pub fn read_log(data_dir: &str, root: &str, id: &str) -> Vec<LogEntry> {
    let path = format!("{}/{id}.jsonl", sessions_dir(data_dir, root));
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().map_while(|l| harness_api::event::from_line(l).ok()).collect()
}

/// This folder's sessions, newest first.
pub fn recent(data_dir: &str, root: &str) -> Vec<SessionInfo> {
    let Ok(files) = std::fs::read_dir(sessions_dir(data_dir, root)) else { return Vec::new() };
    let mut out: Vec<SessionInfo> = files
        .flatten()
        .filter_map(|f| f.file_name().to_str()?.strip_suffix(".jsonl").map(str::to_string))
        .filter_map(|id| {
            let log = read_log(data_dir, root, &id);
            let title = log.iter().find_map(|e| match &e.event {
                Event::UserMessage { text } => text.lines().next().map(str::to_string),
                _ => None,
            })?;
            let model = log.iter().rev().find_map(|e| match &e.event {
                Event::TurnStarted { model, .. } => Some(model.clone()),
                _ => None,
            });
            Some(SessionInfo { id, title, model: model.unwrap_or_default(), last_ms: log.last().map_or(0, |e| e.at_ms) })
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.last_ms));
    out
}

/// Builds a session's parts, on the session's own thread: they are not `Send`.
pub type Factory = Arc<dyn Fn(u64) -> Result<Parts, String> + Send + Sync>;

/// The real plugins, chosen by the configuration.
pub fn real_factory(settings: Settings, root: String, data_dir: PathBuf) -> Factory {
    with_host(settings, root, data_dir, Host::default())
}

/// What the running binary gives its sessions.
#[derive(Clone)]
pub struct Host {
    /// A line for a tab's system prompt: where its callbacks go.
    pub note: Arc<dyn Fn(u64) -> Option<String> + Send + Sync>,
    /// What `plugins_list` answers.
    pub plugins: Arc<dyn Fn() -> Value + Send + Sync>,
    /// `plugins_reload` raises this; the frontend does the rest at a safe point.
    pub reload: Arc<std::sync::atomic::AtomicBool>,
}

impl Default for Host {
    fn default() -> Self {
        Host { note: Arc::new(|_| None), plugins: Arc::new(|| json!({})), reload: Default::default() }
    }
}

pub fn with_host(settings: Settings, root: String, data_dir: PathBuf, host: Host) -> Factory {
    Arc::new(move |tab| {
        let data = data_dir.to_string_lossy().into_owned();
        let mut platform = harness_platform_native::platform(data_dir.clone());
        if let Some(path) = wire_log_path() {
            platform.net = Rc::new(wire::WireLog::new(platform.net.clone(), path));
        }
        let mut selection = settings.selection();
        selection.tools.extend(["plugins_list".to_string(), "plugins_reload".to_string(), "debug_contract".to_string(), "debug_runs".to_string()]);
        let root_debug = root.clone();

        let mut registry = registry(&settings, &root, &data, (host.note)(tab));
        let (plugins, reload) = (host.plugins.clone(), host.reload.clone());
        registry.tool("plugins_list", move |_| Ok(Rc::new(host_tools::PluginsList(plugins.clone()))));
        registry.tool("plugins_reload", move |_| Ok(Rc::new(host_tools::PluginsReload(reload.clone()))));
        registry.tool("debug_contract", |_| Ok(Rc::new(host_tools::DebugContract)));
        registry.tool("debug_runs", move |_| Ok(Rc::new(host_tools::DebugRuns { root: root_debug.clone() })));
        // Offered only when the daemon's channel is on: a tool that cannot work is noise.
        let telegram = harness_daemon::client::Client::find(&data_dir).and_then(|c| c.stats().ok()).is_some_and(|s| s["telegram"]["on"] == true);
        if telegram {
            selection.tools.push("telegram_send".to_string());
            let data_dir = data_dir.clone();
            registry.tool("telegram_send", move |_| Ok(Rc::new(host_tools::TelegramSend { data_dir: data_dir.clone() })));
        }
        registry.resolve(&selection, platform).map_err(|e| e.to_string())
    })
}

/// What a running session tells the frontend.
#[derive(Debug, Clone, PartialEq)]
pub enum FromSession {
    Entry { tab: u64, entry: LogEntry },
    /// `Policy` asked; the session waits for `Sessions::answer`.
    Ask { tab: u64, call: ToolCall, capabilities: Vec<Capability> },
    /// One message's run ended.
    Ended { tab: u64, end: RunEnd },
    /// The session could not start.
    Unavailable { tab: u64, message: String },
}

/// What a session's next run starts from.
enum Input {
    Prompt(String),
    Task { text: String, protocol: Protocol },
    Plan(bool),
    StepDebug(bool),
    Inbound { label: String, text: String },
}

struct Handle {
    session: String,
    prompts: Sender<Input>,
    answers: Sender<Answer>,
    stop: StopFlag,
}

/// The sessions behind the tabs: one thread each, started by the tab's first message.
/// A project's `.harness/acceptance.toml`: specifications as checks (ADR 0007). Commands live
/// here, in a file the user owns, and never in journal entries: a callback or a model can write
/// those, and running them would let a payload choose what runs (INV-12).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    #[serde(default)]
    check: Vec<AcceptanceCheck>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceCheck {
    spec: u64,
    command: String,
}

/// The journal of a project, opened directly: for a look from the command line.
pub fn project_memory(data_dir: &str, root: &str) -> Result<harness_memory_journal::plugin::JournalMemory, String> {
    let key = harness_memory_journal::project::key_for(git_origin(root).as_deref(), std::path::Path::new(root));
    harness_memory_journal::plugin::JournalMemory::open(&std::path::Path::new(data_dir).join("journals"), &key, "multitool")
}

/// Whether to ask about initializing this project: true once, when it has neither `docs/` nor
/// `.harness/` (CON-44). Asking is remembered, so the question does not come back.
pub fn first_start_asks(data_dir: &str, root: &str) -> bool {
    let at = std::path::Path::new(root);
    if at.join("docs").exists() || at.join(".harness").exists() {
        return false;
    }
    let marker = format!("{}.init-asked", sessions_dir(data_dir, root));
    if std::path::Path::new(&marker).exists() {
        return false;
    }
    if let Some(dir) = std::path::Path::new(&marker).parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(&marker, "").is_ok()
}

/// Why a task did not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskRefused {
    Invalid(String),
    /// The project's checks are new or changed: `text` is the file as read, to approve as shown.
    NotApproved { commands: Vec<String>, text: String },
}

impl std::fmt::Display for TaskRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskRefused::Invalid(reason) => f.write_str(reason),
            TaskRefused::NotApproved { commands, .. } => write!(f, "this project's checks are not approved yet; they would run: {}", commands.join("; ")),
        }
    }
}

/// The copy of a project's acceptance file the user approved.
fn approved_path(data_dir: &str, root: &str) -> String {
    format!("{}.acceptance.toml", sessions_dir(data_dir, root))
}

/// Approves this text of the project's checks, the one the user was shown.
pub fn approve_acceptance(data_dir: &str, root: &str, text: &str) -> Result<(), String> {
    let path = approved_path(data_dir, root);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))
}

/// The protocol of a task in this project: its own steps, which the user just gave, then the
/// acceptance checks, which a repository brings and so run only as approved (CON-51). The file
/// is read once, so what was compared is what runs. No data directory means nothing is approved.
pub fn protocol_for(data_dir: Option<&str>, root: &str, mut steps: Vec<String>) -> Result<Protocol, TaskRefused> {
    let path = std::path::Path::new(root).join(".harness").join("acceptance.toml");
    let mut specs = Vec::new();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let file: Acceptance = toml::from_str(&text).map_err(|e| TaskRefused::Invalid(format!("{}: {e}", path.display())))?;
            let approved = data_dir.and_then(|d| std::fs::read_to_string(approved_path(d, root)).ok());
            if approved.as_deref() != Some(text.as_str()) {
                return Err(TaskRefused::NotApproved { commands: file.check.into_iter().map(|c| c.command).collect(), text });
            }
            for check in file.check {
                specs.push(check.spec);
                steps.push(check.command);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(TaskRefused::Invalid(format!("{}: {e}", path.display()))),
    }
    if steps.is_empty() {
        return Err(TaskRefused::Invalid("a task needs something to check: a command, or .harness/acceptance.toml".to_string()));
    }
    Ok(Protocol { steps, specs })
}

pub struct Sessions {
    factory: Factory,
    root: String,
    data_dir: Option<String>,
    handles: HashMap<u64, Handle>,
    to_frontend: Sender<FromSession>,
    from_sessions: Receiver<FromSession>,
    started_ms: u64,
}

struct ChannelFrontend {
    tab: u64,
    to_frontend: Sender<FromSession>,
    answers: Receiver<Answer>,
}

impl Frontend for ChannelFrontend {
    fn on_entry(&self, entry: &LogEntry) {
        let _ = self.to_frontend.send(FromSession::Entry { tab: self.tab, entry: entry.clone() });
    }

    fn ask<'a>(&'a self, call: &'a ToolCall, capabilities: &'a [Capability]) -> BoxFuture<'a, Answer> {
        let _ = self.to_frontend.send(FromSession::Ask { tab: self.tab, call: call.clone(), capabilities: capabilities.to_vec() });
        // No frontend left means nobody can approve: deny.
        Box::pin(std::future::ready(self.answers.recv().unwrap_or(Answer::Deny)))
    }
}

impl Sessions {
    pub fn new(factory: Factory, root: String) -> Self {
        let (to_frontend, from_sessions) = channel();
        let started_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        Sessions { factory, root, data_dir: None, handles: HashMap::new(), to_frontend, from_sessions, started_ms }
    }

    /// Where approvals are kept; without it a project's checks never run.
    pub fn with_data_dir(mut self, data_dir: &str) -> Self {
        self.data_dir = Some(data_dir.to_string());
        self
    }

    /// Approves the project's checks as they were shown in a refusal.
    pub fn approve_acceptance(&self, text: &str) -> Result<(), String> {
        let data_dir = self.data_dir.as_deref().ok_or("this frontend keeps no approvals")?;
        approve_acceptance(data_dir, &self.root, text)
    }

    /// The stored session behind a tab, once it has one.
    pub fn session_of(&self, tab: u64) -> Option<&str> {
        self.handles.get(&tab).map(|h| h.session.as_str())
    }

    /// New sessions start from this; running ones keep what they have.
    pub fn set_factory(&mut self, factory: Factory) {
        self.factory = factory;
    }

    /// Binds a tab to a stored session: its next message continues that log.
    pub fn resume(&mut self, tab: u64, session: &str) {
        self.start(tab, Some(session.to_string()));
    }

    pub fn prompt(&mut self, tab: u64, text: String) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Prompt(text));
        }
    }

    /// A message that is a task: done only when `steps` and the project's acceptance checks pass
    /// (INV-4). Refused, with nothing started, if there is nothing to check or the file is bad.
    pub fn task(&mut self, tab: u64, text: String, steps: Vec<String>) -> Result<(), TaskRefused> {
        let protocol = protocol_for(self.data_dir.as_deref(), &self.root, steps)?;
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Task { text, protocol });
        }
        Ok(())
    }

    /// The user switches step-debug mode.
    pub fn step_debug(&mut self, tab: u64, on: bool) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::StepDebug(on));
        }
    }

    /// The user switches plan mode. It takes effect when the running turn, if any, has ended.
    pub fn plan(&mut self, tab: u64, on: bool) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Plan(on));
        }
    }

    /// A callback into the agent's input; it waits its turn if the agent is busy.
    pub fn inbound(&mut self, tab: u64, label: String, text: String) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Inbound { label, text });
        }
    }

    pub fn answer(&self, tab: u64, answer: Answer) {
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.answers.send(answer);
        }
    }

    pub fn stop(&self, tab: u64) {
        if let Some(handle) = self.handles.get(&tab) {
            handle.stop.stop();
        }
    }

    /// Everything the sessions have said since the last call.
    pub fn drain(&self) -> Vec<FromSession> {
        self.from_sessions.try_iter().collect()
    }

    /// Waits for the next message, for callers with nothing else to do.
    pub fn next(&self, timeout: std::time::Duration) -> Option<FromSession> {
        self.from_sessions.recv_timeout(timeout).ok()
    }

    fn start(&mut self, tab: u64, resume: Option<String>) {
        let (prompts, prompt_rx) = channel::<Input>();
        let (answers, answer_rx) = channel();
        let stop = StopFlag::default();
        let (factory, to_frontend, flag, root) = (self.factory.clone(), self.to_frontend.clone(), stop.clone(), self.root.clone());
        let resuming = resume.is_some();
        let session = resume.unwrap_or_else(|| format!("{}-{tab}", self.started_ms));
        let stored_name = session.clone();
        std::thread::spawn(move || {
            let parts = match factory(tab) {
                Ok(parts) => parts,
                Err(message) => {
                    while prompt_rx.recv().is_ok() {
                        let _ = to_frontend.send(FromSession::Unavailable { tab, message: message.clone() });
                    }
                    return;
                }
            };
            let frontend = Rc::new(ChannelFrontend { tab, to_frontend: to_frontend.clone(), answers: answer_rx });
            let engine = Engine::new(parts, frontend, session, root).with_stop_flag(flag);
            if resuming {
                match pollster::block_on(engine.load_log()) {
                    Ok(entries) => engine.resume(&entries),
                    Err(message) => {
                        let _ = to_frontend.send(FromSession::Unavailable { tab, message });
                    }
                }
            }
            while let Ok(input) = prompt_rx.recv() {
                if let Input::Plan(on) = input {
                    pollster::block_on(engine.set_plan_mode(on, "user"));
                    continue;
                }
                if let Input::StepDebug(on) = input {
                    pollster::block_on(engine.set_step_debug(on, "user"));
                    continue;
                }
                let end = match input {
                    Input::Plan(_) | Input::StepDebug(_) => unreachable!("handled above"),
                    Input::Prompt(text) => pollster::block_on(engine.run(&text, None)),
                    Input::Task { text, protocol } => pollster::block_on(engine.run(&text, Some(&protocol))),
                    Input::Inbound { label, text } => pollster::block_on(engine.run_inbound(&label, &text)),
                };
                let _ = to_frontend.send(FromSession::Ended { tab, end });
            }
        });
        self.handles.insert(tab, Handle { session: stored_name, prompts, answers, stop });
    }
}
