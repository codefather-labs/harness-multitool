//! Runs sessions on a native build (roadmap M2): the configuration selects the plugins (IF-0.3),
//! and each session's loop runs on its own thread, talking to the frontend over channels.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// What the hook mode-guide does on a write with no plan: remind (default), enforce or off.
    pub plan_first: Option<String>,
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
            plan_first: None,
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

    /// Whether the chosen provider row is Claude Code as a delegate (ADR 0020).
    fn runs_delegate(&self) -> bool {
        self.providers.get(&self.provider).is_some_and(|p| p["kind"] == "claude-code")
    }

    fn selection(&self) -> Selection {
        Selection {
            auth: "api-key".into(),
            // A delegate is no provider: the engine hands its turns over (IF-17).
            providers: if self.runs_delegate() { Vec::new() } else { vec![self.provider.clone()] },
            router: "one-model".into(),
            tools: self.tools.clone(),
            policy: "default".into(),
            hooks: (0..self.hooks.len()).map(|i| format!("shell-{i}")).chain(["docs-mirror".to_string(), "mode-guide".to_string()]).collect(),
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
    let delegate_session = settings.runs_delegate();
    let context_budget = settings.compact_at_tokens.unwrap_or(settings.context_window() * 7 / 10);
    let sessions_dir = sessions_dir(data_dir, root);
    let (root_d, data_d) = (root.to_string(), std::path::PathBuf::from(data_dir));
    let auth_data = PathBuf::from(data_dir);
    r.auth("api-key", move |cx| Ok(Rc::new(ProviderAuth::new(harness_auth_apikey::ApiKeyAuth::new(env, cx.settings), cx.settings, &auth_data))))
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
        .context("journal-aware", move |_| Ok(Rc::new(harness_context_default::JournalAware { root: root_b.clone(), note: note.clone(), budget_tokens: context_budget, mode_rules: !delegate_session })))
        .store("files", move |cx| Ok(Rc::new(harness_session_files::FileSessions::new(cx.platform.fs.clone(), &sessions_dir))))
        .verifier("protocol", move |cx| Ok(Rc::new(harness_verifier_protocol::ProtocolRunner::new(cx.platform.process.clone(), &root_c))));
    for name in harness_tools_memory::NAMES {
        r.tool(name, move |cx| {
            let memory = cx.memory.clone().ok_or("the tool was built before Memory")?;
            let tool = harness_tools_memory::tool(memory, name).ok_or("no such memory tool")?;
            Ok(Rc::new(tool))
        });
    }
    // A delegate has none of the tools the guide speaks of.
    let plan_first = if delegate_session { harness_core::plan::PlanFirst::Off } else { harness_core::plan::PlanFirst::parse(settings.plan_first.as_deref()) };
    r.hook("mode-guide", move |_| Ok(Rc::new(harness_core::plan::ModeGuide::new(plan_first))));
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
        if settings.providers.get(name).is_some_and(|p| p["kind"] == "claude-code") {
            continue;
        }
        let kind = settings.providers.get(name).and_then(|p| p["kind"].as_str()).map(str::to_string).unwrap_or_else(|| if name == "anthropic" { "anthropic".to_string() } else { "openai".to_string() });
        let provider_name = name.to_string();
        let on_plan = settings.auth.get(name).is_some_and(|a| a["oauth"] == "chatgpt");
        r.provider(name, move |cx| {
            let auth = cx.auth.clone().ok_or("the provider was built before Auth")?;
            let provider: Rc<dyn Provider> = if kind == "openai-responses" {
                Rc::new(harness_provider_openai::responses::Responses::new(&provider_name, auth, cx.platform.net.clone(), cx.settings).on_plan(on_plan))
            } else if kind == "anthropic" {
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

/// Stores a conversation brought in from elsewhere as a session of this project, so it opens like
/// any other. The file appears whole or not at all. Returns the session's id.
pub fn store_imported(data_dir: &str, root: &str, imported: &ImportedConversation, now_ms: u64) -> Result<String, String> {
    let short: String = imported.info.id.chars().take(8).collect();
    let id = format!("imported-{short}-{now_ms}");
    let dir = sessions_dir(data_dir, root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{dir}: {e}"))?;
    let (path, staging) = (format!("{dir}/{id}.jsonl"), format!("{dir}/{id}.partial"));
    let mut text = String::new();
    for entry in &imported.entries {
        text.push_str(&harness_api::event::to_line(entry));
        text.push('\n');
    }
    std::fs::write(&staging, text).map_err(|e| format!("{staging}: {e}"))?;
    std::fs::rename(&staging, &path).map_err(|e| format!("{path}: {e}"))?;
    Ok(id)
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
        let mut parts = registry.resolve(&selection, platform).map_err(|e| e.to_string())?;
        if settings.runs_delegate() {
            parts.backend = Some(delegate(&settings, &root)?);
        }
        Ok(parts)
    })
}

/// Claude Code as a delegate, in a build that has it (ADR 0020).
#[cfg(feature = "claude-code")]
fn delegate(settings: &Settings, root: &str) -> Result<Rc<dyn AgentBackend>, String> {
    let row = settings.providers.get(&settings.provider).cloned().unwrap_or_default();
    Ok(Rc::new(harness_backend_claude_code::ClaudeCode::new(&settings.model, root, &row)))
}

/// Where conversations held by another agent are read from, in a build that has one (IF-18).
#[cfg(feature = "claude-code")]
pub fn conversation_source(name: &str) -> Result<Box<dyn ConversationSource>, String> {
    match name {
        harness_backend_claude_code::NAME => Ok(Box::new(harness_backend_claude_code::conversations::ClaudeCodeConversations::from_env().map_err(|e| e.to_string())?)),
        other => Err(format!("no conversations from {other}; known: {}", harness_backend_claude_code::NAME)),
    }
}

#[cfg(not(feature = "claude-code"))]
pub fn conversation_source(_name: &str) -> Result<Box<dyn ConversationSource>, String> {
    Err("this build reads no conversations: build with --features claude-code".to_string())
}

#[cfg(not(feature = "claude-code"))]
fn delegate(_settings: &Settings, _root: &str) -> Result<Rc<dyn AgentBackend>, String> {
    Err("this build has no Claude Code delegate: build with --features claude-code".to_string())
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
    /// Something is in the inbox; if a run took it already, nothing is left to do.
    Next,
    Task { text: String, protocol: Protocol },
    Plan(bool),
    StepDebug(bool),
    Nick(String),
    Inbound { label: String, text: String },
}

struct Handle {
    session: String,
    prompts: Sender<Input>,
    answers: Sender<Answer>,
    stop: StopFlag,
    /// A run is on. Set and read under the inbox lock, so a message has one owner.
    running: Arc<AtomicBool>,
    /// This thread has been replaced by a rebind: it takes no more messages.
    retired: Arc<AtomicBool>,
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

/// Credentials by provider (IF-2): `[session.auth.<provider>] oauth = "chatgpt"` signs in with a
/// ChatGPT plan; anything else is an API key.
struct ProviderAuth {
    keys: harness_auth_apikey::ApiKeyAuth,
    chatgpt: harness_auth_chatgpt::ChatGptAuth,
    on_plan: Vec<String>,
    /// Providers configured with the empty claude-subscription adapter.
    refused: Vec<String>,
}

/// The claude-subscription adapter: it stands in the interface and refuses, whoever configures it,
/// and holds no token logic (CON-56).
pub struct ClaudeSubscription;

impl Auth for ClaudeSubscription {
    fn credentials(&self, _provider: &str) -> BoxFuture<'_, PluginResult<Secret>> {
        Box::pin(std::future::ready(Err(PluginError::new(CLAUDE_SUBSCRIPTION_REFUSED))))
    }
}

impl ProviderAuth {
    fn new(keys: harness_auth_apikey::ApiKeyAuth, settings: &Value, data_dir: &std::path::Path) -> Self {
        let on_plan = settings.as_object().map(|m| m.iter().filter(|(_, v)| v["oauth"] == "chatgpt").map(|(k, _)| k.clone()).collect()).unwrap_or_default();
        let refused = settings.as_object().map(|m| m.iter().filter(|(_, v)| v["oauth"] == "claude-subscription").map(|(k, _)| k.clone()).collect()).unwrap_or_default();
        ProviderAuth { keys, chatgpt: harness_auth_chatgpt::ChatGptAuth::new(data_dir), on_plan, refused }
    }
}

impl Auth for ProviderAuth {
    fn credentials(&self, provider: &str) -> BoxFuture<'_, PluginResult<Secret>> {
        if self.refused.iter().any(|p| p == provider) {
            return ClaudeSubscription.credentials(provider);
        }
        if self.on_plan.iter().any(|p| p == provider) { self.chatgpt.credentials(provider) } else { self.keys.credentials(provider) }
    }
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
    /// Each tab's messages not yet seen by its agent; it outlives the tab's thread.
    inboxes: HashMap<u64, harness_core::Inbox>,
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
        Sessions { inboxes: HashMap::new(), factory, root, data_dir: None, handles: HashMap::new(), to_frontend, from_sessions, started_ms }
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
    /// Moves a tab's session to the current factory, its history kept: what settings changed
    /// reaches a session that has already started. Only between runs.
    ///
    /// False if a run is on after all (a queued message started one): try at the next `Ended`.
    pub fn rebind(&mut self, tab: u64) -> bool {
        let Some(handle) = self.handles.get(&tab) else { return true };
        let inbox = self.inboxes.entry(tab).or_default().clone();
        let queued = {
            let inbox = inbox.lock().expect("inbox");
            if handle.running.load(Ordering::SeqCst) {
                return false;
            }
            handle.retired.store(true, Ordering::SeqCst);
            !inbox.is_empty()
        };
        let Some(handle) = self.handles.remove(&tab) else { return true };
        self.start(tab, Some(handle.session));
        // What waited on the old thread's channel goes on the new one.
        if queued && let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Next);
        }
        true
    }

    pub fn resume(&mut self, tab: u64, session: &str) {
        self.start(tab, Some(session.to_string()));
    }

    pub fn prompt(&mut self, tab: u64, text: String) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            self.inboxes.entry(tab).or_default().lock().expect("inbox").push_back(text);
            let _ = handle.prompts.send(Input::Next);
        }
    }

    /// Takes a queued message back, if the agent has not seen it yet.
    pub fn unqueue(&mut self, tab: u64, text: &str) -> bool {
        let Some(inbox) = self.inboxes.get(&tab) else { return false };
        let mut inbox = inbox.lock().expect("inbox");
        match inbox.iter().position(|t| t == text) {
            Some(at) => inbox.remove(at).is_some(),
            None => false,
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

    /// The user names the agent.
    pub fn nick(&mut self, tab: u64, name: String) {
        if !self.handles.contains_key(&tab) {
            self.start(tab, None);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(Input::Nick(name));
        }
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
        let inbox = self.inboxes.entry(tab).or_default().clone();
        let inbox_for_thread = inbox.clone();
        let (running, retired) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (running_here, retired_here) = (running.clone(), retired.clone());
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
            let engine = Engine::new(parts, frontend, session, root).with_stop_flag(flag).with_inbox(inbox_for_thread);
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
                if let Input::Nick(name) = &input {
                    pollster::block_on(engine.set_agent_name_by_user(name));
                    continue;
                }
                if let Input::StepDebug(on) = input {
                    pollster::block_on(engine.set_step_debug(on, "user"));
                    continue;
                }
                let end = match input {
                    Input::Plan(_) | Input::StepDebug(_) | Input::Nick(_) => unreachable!("handled above"),
                    Input::Next => {
                        let next = {
                            let mut inbox = inbox.lock().expect("inbox");
                            if retired_here.load(Ordering::SeqCst) {
                                break;
                            }
                            let next = inbox.pop_front();
                            running_here.store(next.is_some(), Ordering::SeqCst);
                            next
                        };
                        // Nothing to do when a run took it or it was taken back; that run's own
                        // end has told the frontend, and an `Ended` here could cut a later run.
                        let Some(text) = next else { continue };
                        pollster::block_on(engine.run(&text, None))
                    }
                    Input::Task { text, protocol } => {
                        running_here.store(true, Ordering::SeqCst);
                        pollster::block_on(engine.run(&text, Some(&protocol)))
                    }
                    Input::Inbound { label, text } => {
                        running_here.store(true, Ordering::SeqCst);
                        pollster::block_on(engine.run_inbound(&label, &text))
                    }
                };
                running_here.store(false, Ordering::SeqCst);
                let _ = to_frontend.send(FromSession::Ended { tab, end });
            }
        });
        self.handles.insert(tab, Handle { session: stored_name, prompts, answers, stop, running, retired });
    }
}
