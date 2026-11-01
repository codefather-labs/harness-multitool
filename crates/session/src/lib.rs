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
    pub tools: Vec<String>,
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
            tools: ["read", "edit", "shell", "search", "fetch"].map(String::from).to_vec(),
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
            hooks: vec![],
            memory: "none".into(),
            context: "whole-conversation".into(),
            store: "files".into(),
            verifier: "protocol".into(),
        }
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Everything shipped in this build, under its configuration names.
pub fn registry(settings: &Settings, root: &str, data_dir: &str) -> Registry {
    let mut r = Registry::new();
    let (root_a, root_b, root_c) = (root.to_string(), root.to_string(), root.to_string());
    let sessions_dir = format!("{}/sessions", data_dir.trim_end_matches('/'));
    r.auth("api-key", |cx| Ok(Rc::new(harness_auth_apikey::ApiKeyAuth::new(env, cx.settings))))
        .router("one-model", |cx| Ok(Rc::new(harness_router_default::OneModel::new(cx.settings)?)))
        .tool("read", |_| Ok(Rc::new(harness_tools_builtin::Read)))
        .tool("edit", |_| Ok(Rc::new(harness_tools_builtin::Edit)))
        .tool("shell", |_| Ok(Rc::new(harness_tools_builtin::Shell)))
        .tool("search", |_| Ok(Rc::new(harness_tools_builtin::Search)))
        .tool("fetch", |_| Ok(Rc::new(harness_tools_builtin::Fetch)))
        .policy("default", move |_| Ok(Rc::new(harness_policy_default::DefaultPolicy::new(&root_a))))
        .memory("none", |_| Ok(Rc::new(harness_context_default::NoMemory)))
        .context("whole-conversation", move |_| Ok(Rc::new(harness_context_default::WholeConversation { root: root_b.clone() })))
        .store("files", move |cx| Ok(Rc::new(harness_session_files::FileSessions::new(cx.platform.fs.clone(), &sessions_dir))))
        .verifier("protocol", move |cx| Ok(Rc::new(harness_verifier_protocol::ProtocolRunner::new(cx.platform.process.clone(), &root_c))));
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

/// Builds a session's parts, on the session's own thread: they are not `Send`.
pub type Factory = Arc<dyn Fn() -> Result<Parts, String> + Send + Sync>;

/// The real plugins, chosen by the configuration.
pub fn real_factory(settings: Settings, root: String, data_dir: PathBuf) -> Factory {
    Arc::new(move || {
        let data = data_dir.to_string_lossy().into_owned();
        let platform = harness_platform_native::platform(data_dir.clone());
        registry(&settings, &root, &data).resolve(&settings.selection(), platform).map_err(|e| e.to_string())
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

struct Handle {
    prompts: Sender<String>,
    answers: Sender<Answer>,
    stop: StopFlag,
}

/// The sessions behind the tabs: one thread each, started by the tab's first message.
pub struct Sessions {
    factory: Factory,
    root: String,
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
        Sessions { factory, root, handles: HashMap::new(), to_frontend, from_sessions, started_ms }
    }

    pub fn prompt(&mut self, tab: u64, text: String) {
        if !self.handles.contains_key(&tab) {
            self.start(tab);
        }
        if let Some(handle) = self.handles.get(&tab) {
            let _ = handle.prompts.send(text);
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

    fn start(&mut self, tab: u64) {
        let (prompts, prompt_rx) = channel::<String>();
        let (answers, answer_rx) = channel();
        let stop = StopFlag::default();
        let (factory, to_frontend, flag, root) = (self.factory.clone(), self.to_frontend.clone(), stop.clone(), self.root.clone());
        let session = format!("{}-{tab}", self.started_ms);
        std::thread::spawn(move || {
            let parts = match factory() {
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
            while let Ok(text) = prompt_rx.recv() {
                let end = pollster::block_on(engine.run(&text, None));
                let _ = to_frontend.send(FromSession::Ended { tab, end });
            }
        });
        self.handles.insert(tab, Handle { prompts, answers, stop });
    }
}
