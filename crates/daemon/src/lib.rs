//! The daemon (CON-20): the single entry point of callbacks to agents, with a callback URL for
//! every open window (IF-13). A callback carries `uuid` and `type`; its type picks a handler
//! (IF-16). Its payload is data and cannot choose anything else (INV-12). The same uuid is
//! handled once (INV-14); a type with no handler reaches no one, is logged, and gets 400.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_api::{TELEGRAM_LABEL, Callback, CallbackHandler, HandlerEffect, JournalEntry, JournalRecord, MapData, Memory};
use harness_memory_journal::plugin::JournalMemory;
use harness_memory_journal::project::key_for;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod client;
pub mod conditions;
pub mod memory;
pub mod stepdebug;
pub mod telegram;

/// A window that polled within this long is still there.
const LISTENING: std::time::Duration = std::time::Duration::from_secs(60);

/// How long `GET /events` holds a request open when there is nothing to say.
const POLL: Duration = Duration::from_secs(20);

/// What a window hears from the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "event")]
pub enum WindowEvent {
    /// Goes into the agent's input, marked as external data.
    Notification { uuid: String, label: String, text: String },
    /// A `journal` callback wrote an entry; no agent turn.
    Journal { record: JournalRecord },
    /// A `flowstate_update` moved an instance of a chart; no agent turn.
    Flow {
        map: MapData,
        instance: String,
        node: u64,
        state: String,
        off_chart: bool,
        path: Vec<u64>,
        /// This event ended a cycle in step-debug mode, and the run was saved under this id.
        #[serde(default)]
        run: Option<i64>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Numbered {
    pub seq: u64,
    pub event: WindowEvent,
}

struct Window {
    id: String,
    root: String,
    events: Vec<Numbered>,
    /// Each instance's path through each chart: (map, instance) -> nodes in order.
    instances: HashMap<(u64, String), Vec<u64>>,
    /// Step-debug mode: finished runs are saved (ADR 0013). Set by the session, never by a payload.
    step_debug: bool,
    /// The steps of each run so far, for saving it (INV-13).
    cycles: HashMap<(u64, String), Vec<stepdebug::Step>>,
    conditions: Vec<conditions::Armed>,
    /// Standard types switched off for this project.
    off: HashSet<String>,
    /// What this project chose: standard type -> implementation; and what is switched off.
    project: conditions::ProjectCallbacks,
}

struct State {
    /// When each window last asked for events: a window that stopped asking has gone away.
    polled: HashMap<String, std::time::Instant>,
    windows: HashMap<String, Window>,
    seen: HashSet<String>,
    next_seq: u64,
    shutting_down: bool,
}

/// A handler the build brought. It is shared between the server's threads.
pub type SharedHandler = Arc<dyn CallbackHandler + Send + Sync>;

/// IF-19: the conversations the daemon runs. It knows them only through this; the implementation
/// is plugged in by the program that starts it (ADR 0024).
pub trait ConversationHost: Send + Sync {
    /// One operation of a window, as JSON. Never blocks on a model.
    fn op(&self, window: &str, op: &Value) -> Result<Value, String>;
    /// What happened in this window after `after`, waiting up to `wait` for something.
    fn events(&self, window: &str, after: u64, wait: std::time::Duration) -> Vec<Value>;
    fn stats(&self) -> Value;
    /// Ends every conversation and whatever process it holds, unless `only_if_idle` and a run is
    /// on, which answers false and changes nothing.
    fn close(&self, only_if_idle: bool) -> bool;
    /// The sessions open in its windows, for a channel to choose from.
    fn open_sessions(&self) -> Vec<OpenSession> {
        Vec::new()
    }
    /// A message for this session as an inbound. False if it is not open.
    fn deliver_to(&self, _session: &str, _label: &str, _text: &str) -> bool {
        false
    }
    /// Something for the user to read in this session; the agent is not told. False if it is not open.
    fn notify(&self, _session: &str, _label: &str, _text: &str) -> bool {
        false
    }
    /// The session behind a window's tab.
    fn session_of(&self, _window: &str, _tab: u64) -> Option<String> {
        None
    }
    /// Names a session. False if it is not open.
    fn rename(&self, _session: &str, _nick: &str) -> bool {
        false
    }
}

/// The Telegram channel's settings, as they stand when asked.
pub type TelegramSource = Arc<dyn Fn() -> Option<telegram::Config> + Send + Sync>;

/// A session open in a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSession {
    pub id: String,
    /// What a person knows it by.
    pub title: String,
    /// The name it goes by, if it has one.
    pub nick: Option<String>,
}

/// The path to run this program by. A rebuild puts a new file at that path, after which the OS
/// names the running one as deleted: the path still holds the program, in its newer build.
pub fn own_program() -> std::io::Result<std::path::PathBuf> {
    std::env::current_exe().map(live_path)
}

fn live_path(path: std::path::PathBuf) -> std::path::PathBuf {
    if path.exists() {
        return path;
    }
    match path.to_str().and_then(|p| p.strip_suffix(" (deleted)")).map(std::path::PathBuf::from) {
        Some(live) if live.exists() => live,
        _ => path,
    }
}

#[cfg(test)]
mod own_program_tests {
    #[test]
    fn a_replaced_program_runs_by_its_path() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("multitool");
        let named = |suffix: &str| std::path::PathBuf::from(format!("{}{suffix}", live.display()));
        assert_eq!(super::live_path(named(" (deleted)")), named(" (deleted)"), "gone for good: nothing better to say");
        std::fs::write(&live, "").unwrap();
        assert_eq!(super::live_path(named(" (deleted)")), live);
        assert_eq!(super::live_path(live.clone()), live);
    }
}

/// What identifies this binary: a client of another build must not be served by this one.
pub fn build_id() -> String {
    std::env::current_exe().ok().and_then(|p| std::fs::metadata(p).ok()).map_or(String::new(), |m| {
        let modified = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
        format!("{}-{modified}", m.len())
    })
}

pub struct Daemon {
    conversations: Mutex<Option<Arc<dyn ConversationHost>>>,
    handlers: Vec<SharedHandler>,
    state: Mutex<State>,
    wake: Condvar,
    admin: String,
    data_dir: PathBuf,
    log: Mutex<std::fs::File>,
    port: u16,
    /// Memory calls served, for `multitool daemon --status`.
    memory_ops: std::sync::atomic::AtomicU64,
    telegram: Mutex<Option<telegram::Channel>>,
    telegram_generation: std::sync::atomic::AtomicU64,
    telegram_source: Mutex<Option<TelegramSource>>,
    channels_stopped: std::sync::atomic::AtomicBool,
}

/// Where a running daemon says how to reach it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Address {
    pub port: u16,
    pub admin: String,
    /// The daemon's process, so that a stop hits exactly it.
    #[serde(default)]
    pub pid: u32,
}

pub fn address_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("address.json")
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A file only its owner can read, for the admin token.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    std::fs::write(path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

impl Daemon {
    /// Binds a localhost port (`0` picks one), publishes the address, and serves on a background
    /// thread pool until the process ends.
    pub fn start(data_dir: &Path, port: u16) -> Result<Arc<Daemon>, String> {
        Self::start_with(data_dir, port, Vec::new())
    }

    /// The same, with the custom handlers of this build (IF-16).
    pub fn start_with(data_dir: &Path, port: u16, handlers: Vec<SharedHandler>) -> Result<Arc<Daemon>, String> {
        let dir = data_dir.join("daemon");
        std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let server = Arc::new(tiny_http::Server::http(("127.0.0.1", port)).map_err(|e| format!("binding a port: {e}"))?);
        let port = server.server_addr().to_ip().map(|a| a.port()).ok_or("the server has no port")?;

        let log_path = dir.join("triggers.jsonl");
        let seen = std::fs::read_to_string(&log_path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|v| v["outcome"] == "handled")
            .filter_map(|v| v["uuid"].as_str().map(str::to_string))
            .collect();
        let log = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).map_err(|e| format!("opening the trigger log: {e}"))?;

        let admin = uuid::Uuid::new_v4().to_string();
        let address = serde_json::to_string(&Address { port, admin: admin.clone(), pid: std::process::id() }).expect("an address serialises");
        write_private(&address_path(data_dir), &address).map_err(|e| format!("publishing the address: {e}"))?;

        let daemon = Arc::new(Daemon {
            conversations: Mutex::new(None),
            handlers,
            // Numbered from the clock: a window listening across a restart still sees new events.
            state: Mutex::new(State { polled: HashMap::new(), windows: restore_windows(&dir), seen, next_seq: now_ms(), shutting_down: false }),
            wake: Condvar::new(),
            admin,
            data_dir: data_dir.to_path_buf(),
            log: Mutex::new(log),
            port,
            memory_ops: std::sync::atomic::AtomicU64::new(0),
            telegram: Mutex::new(None),
            telegram_generation: Default::default(),
            telegram_source: Mutex::new(None),
            channels_stopped: std::sync::atomic::AtomicBool::new(false),
        });
        // Long polls hold a thread each, so serve from several.
        for _ in 0..8 {
            let (server, daemon) = (server.clone(), daemon.clone());
            std::thread::spawn(move || {
                while let Ok(request) = server.recv() {
                    daemon.serve(request);
                }
            });
        }
        Ok(daemon)
    }

    /// From now on the conversations of its windows run here.
    pub fn host_conversations(&self, host: Arc<dyn ConversationHost>) {
        *self.conversations.lock().expect("conversations") = Some(host);
    }

    fn conversation_op(&self, body: &str) -> (u16, Value) {
        let Some(host) = self.conversations.lock().expect("conversations").clone() else { return (503, json!({"error": "this daemon runs no conversations"})) };
        let Ok(input) = serde_json::from_str::<Value>(body) else { return (400, json!({"error": "not JSON"})) };
        let Some(window) = input["window"].as_str() else { return (400, json!({"error": "window is required"})) };
        match host.op(window, &input) {
            Ok(reply) => (200, reply),
            Err(error) => (400, json!({"error": error})),
        }
    }

    fn conversation_events(&self, query: &str) -> (u16, Value) {
        let Some(host) = self.conversations.lock().expect("conversations").clone() else { return (503, json!({"error": "this daemon runs no conversations"})) };
        let param = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('='));
        let Some(window) = param("window") else { return (400, json!({"error": "window is required"})) };
        let after = param("after").and_then(|a| a.parse().ok()).unwrap_or(0);
        (200, json!({"events": host.events(window, after, std::time::Duration::from_secs(2))}))
    }

    /// Starts the Telegram channel. With no call to this it stays off and nothing polls.
    pub fn start_telegram(self: &Arc<Self>, config: telegram::Config) {
        let generation = self.telegram_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        *self.telegram.lock().expect("telegram") = Some(telegram::Channel::new(config));
        let daemon = self.clone();
        std::thread::spawn(move || telegram::run(daemon, generation));
    }

    /// Where the channel's settings come from: read now, and again when the user saves them.
    pub fn telegram_from(self: &Arc<Self>, source: TelegramSource) {
        *self.telegram_source.lock().expect("telegram source") = Some(source);
        self.reload_telegram();
    }

    /// The channel as its settings now say: the old one stops polling, and with no token none starts.
    fn reload_telegram(self: &Arc<Self>) -> (u16, Value) {
        let source = self.telegram_source.lock().expect("telegram source").clone();
        match source.and_then(|source| source()) {
            Some(config) => self.start_telegram(config),
            None => {
                self.telegram_generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                *self.telegram.lock().expect("telegram") = None;
            }
        }
        (200, self.stats()["telegram"].clone())
    }

    /// Channels stop taking messages; a restart picks up where they stopped.
    pub fn stop_channels(&self) {
        self.channels_stopped.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn conversations(&self) -> Option<Arc<dyn ConversationHost>> {
        self.conversations.lock().expect("conversations").clone()
    }

    fn with_channel(&self, change: impl FnOnce(&mut telegram::Channel)) {
        if let Some(channel) = self.telegram.lock().expect("telegram").as_mut() {
            change(channel);
        }
    }

    /// The window that polled last, if one did within `LISTENING`.
    fn listening_window(&self) -> Option<String> {
        let state = self.state.lock().expect("daemon state");
        state.polled.iter().filter(|(id, at)| at.elapsed() < LISTENING && state.windows.contains_key(*id)).max_by_key(|(_, at)| **at).map(|(id, _)| id.clone())
    }

    fn stats(&self) -> Value {
        let telegram = self.telegram.lock().expect("telegram").as_ref().map_or(json!({"on": false}), telegram::Channel::status);
        json!({"conversations": self.conversations.lock().expect("conversations").as_ref().map_or(Value::Null, |h| h.stats()), "memory_ops": self.memory_ops.load(std::sync::atomic::Ordering::Relaxed), "pid": std::process::id(), "port": self.port, "telegram": telegram, "step_debug_windows": self.state.lock().expect("daemon state").windows.values().filter(|w| w.step_debug).count()})
    }

    /// The session's mode, from its log. Kept in the windows file, so a restart holds it.
    fn set_step_debug(&self, body: &str) -> (u16, Value) {
        let input: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let (Some(window), Some(on)) = (input["window"].as_str(), input["on"].as_bool()) else {
            return (400, json!({"error": "window and on are required"}));
        };
        let mut state = self.state.lock().expect("daemon state");
        let Some(w) = state.windows.get_mut(window) else { return (404, json!({"error": "no such window"})) };
        w.step_debug = on;
        save_windows(&self.data_dir.join("daemon"), &state.windows);
        (200, json!({}))
    }

    fn telegram_send(&self, body: &str) -> (u16, Value) {
        let Some(text) = serde_json::from_str::<Value>(body).ok().and_then(|v| v["text"].as_str().map(str::to_string)) else {
            return (400, json!({"error": "text is required"}));
        };
        let input = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
        // The session asking, if it says: it answers where it is bound.
        let session = input["window"].as_str().zip(input["tab"].as_u64()).and_then(|(window, tab)| self.conversations.lock().expect("conversations").as_ref()?.session_of(window, tab));
        match telegram::send(self, &text, session.as_deref()) {
            Ok(parts) => (200, json!({"parts": parts})),
            Err(error) => (500, json!({"error": error})),
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Blocks until a client asks the daemon to stop; the hot reload does, to replace it.
    pub fn wait_for_shutdown(&self) {
        let mut state = self.state.lock().expect("daemon state");
        while !state.shutting_down {
            state = self.wake.wait(state).expect("daemon state");
        }
    }

    pub fn admin_token(&self) -> &str {
        &self.admin
    }

    fn serve(self: &Arc<Self>, mut request: tiny_http::Request) {
        let url = request.url().to_string();
        let (path, query) = url.split_once('?').unwrap_or((&url, ""));
        let method = request.method().to_string();
        let bearer = request.headers().iter().find(|h| h.field.equiv("authorization")).map(|h| h.value.as_str().trim_start_matches("Bearer ").to_string());
        let admin = bearer.as_deref() == Some(self.admin.as_str());
        let mut body = String::new();
        if std::io::Read::read_to_string(request.as_reader(), &mut body).is_err() {
            body.clear();
        }
        let (status, reply) = match (method.as_str(), path) {
            ("GET", "/health") => (200, json!({"ok": true, "build": build_id()})),
            ("POST", "/conversations") if admin => self.conversation_op(&body),
            ("GET", "/conversations/events") if admin => self.conversation_events(query),
            ("POST", "/conversations") | ("GET", "/conversations/events") => (401, json!({"error": "bad token"})),
            ("POST", "/register") if admin => self.register(&body),
            ("GET", "/events") if admin => self.events(query),
            ("POST", "/reload") if admin => self.reload(),
            ("POST", "/memory") if admin => self.memory_op(&body),
            ("GET", "/stats") if admin => (200, self.stats()),
            ("POST", "/telegram/send") if admin => self.telegram_send(&body),
            ("POST", "/telegram/reload") if admin => self.reload_telegram(),
            ("POST", "/window/step-debug") if admin => self.set_step_debug(&body),
            ("POST", "/window/step-debug") => (401, json!({"error": "bad token"})),
            ("POST", "/memory") | ("GET", "/stats") | ("POST", "/telegram/send") | ("POST", "/telegram/reload") => (401, json!({"error": "bad token"})),
            ("POST", "/shutdown") if admin => {
                let only_if_idle = serde_json::from_str::<Value>(&body).is_ok_and(|b| b["if_idle"] == true);
                let host = self.conversations.lock().expect("conversations").clone();
                if host.is_none_or(|host| host.close(only_if_idle)) {
                    self.state.lock().expect("daemon state").shutting_down = true;
                    self.wake.notify_all();
                    (200, json!({"ok": true}))
                } else {
                    (409, json!({"busy": true}))
                }
            }
            ("GET", "/handlers") if admin => (200, json!({"handlers": self.handlers.iter().map(|h| json!({"name": h.name(), "types": h.types()})).collect::<Vec<_>>()})),
            ("GET", "/handlers") => (401, json!({"error": "bad token"})),
            ("POST", "/register") | ("POST", "/reload") | ("POST", "/shutdown") | ("GET", "/events") => (401, json!({"error": "bad token"})),
            ("POST", p) if p.starts_with("/callback/") => self.callback(&p["/callback/".len()..], &body),
            _ => (404, json!({"error": "no such route"})),
        };
        let response = tiny_http::Response::from_string(reply.to_string())
            .with_status_code(status)
            .with_header(tiny_http::Header::from_bytes(&b"content-type"[..], &b"application/json"[..]).expect("a valid header"));
        let _ = request.respond(response);
    }

    fn register(&self, body: &str) -> (u16, Value) {
        let Ok(input) = serde_json::from_str::<Value>(body) else { return (400, json!({"error": "not JSON"})) };
        let Some(root) = input["root"].as_str() else { return (400, json!({"error": "root is required"})) };
        let id = uuid::Uuid::new_v4().to_string();
        // A broken conditions file does not cost the window; the agent hears about it.
        let (armed, problem) = match conditions::load(root) {
            Ok(armed) => (armed, None),
            Err(message) => (Vec::new(), Some(message)),
        };
        let window = Window { id: id.clone(), root: root.to_string(), events: Vec::new(), instances: HashMap::new(), step_debug: false, cycles: HashMap::new(), conditions: armed, off: off_types(root), project: conditions::read_project(root).unwrap_or_default() };
        {
            let mut state = self.state.lock().expect("daemon state");
            state.windows.insert(id.clone(), window);
            save_windows(&self.data_dir.join("daemon"), &state.windows);
        }
        (200, json!({"window": id, "url": format!("http://127.0.0.1:{}/callback/{id}", self.port), "problem": problem}))
    }

    /// `POST /reload`: every window reads its project's callback files again. Signals gathered
    /// so far start over.
    fn reload(&self) -> (u16, Value) {
        let mut state = self.state.lock().expect("daemon state");
        let mut problems = Vec::new();
        for w in state.windows.values_mut() {
            match conditions::load(&w.root) {
                Ok(armed) => w.conditions = armed,
                Err(message) => problems.push(message),
            }
            w.off = off_types(&w.root);
            w.project = conditions::read_project(&w.root).unwrap_or_default();
        }
        (200, json!({"reloaded": state.windows.len(), "problems": problems}))
    }

    /// `GET /events?window=W&after=N`: what the window has not heard yet, waiting a while for it.
    fn events(&self, query: &str) -> (u16, Value) {
        let param = |name: &str| query.split('&').find_map(|kv| kv.strip_prefix(name)?.strip_prefix('=').map(str::to_string));
        let (Some(window), after) = (param("window"), param("after").and_then(|a| a.parse::<u64>().ok()).unwrap_or(0)) else {
            return (400, json!({"error": "window is required"}));
        };
        let mut state = self.state.lock().expect("daemon state");
        let deadline = std::time::Instant::now() + POLL;
        loop {
            if !state.windows.contains_key(&window) {
                return (404, json!({"error": "no such window"}));
            }
            state.polled.insert(window.clone(), std::time::Instant::now());
            let w = &state.windows[&window];
            let fresh: Vec<&Numbered> = w.events.iter().filter(|e| e.seq > after).collect();
            if !fresh.is_empty() {
                return (200, json!({"events": fresh}));
            }
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return (200, json!({"events": []}));
            }
            state = self.wake.wait_timeout(state, left).expect("daemon state").0;
        }
    }

    fn trigger_log(&self, uuid: &str, kind: &str, window: &str, outcome: &str, detail: &str) {
        let line = json!({"at_ms": now_ms(), "uuid": uuid, "type": kind, "window": window, "outcome": outcome, "detail": detail});
        if let Ok(mut log) = self.log.lock() {
            let _ = writeln!(log, "{line}");
        }
    }

    fn deliver(&self, window: &str, event: WindowEvent) {
        let mut state = self.state.lock().expect("daemon state");
        let seq = state.next_seq;
        state.next_seq += 1;
        if let Some(w) = state.windows.get_mut(window) {
            w.events.push(Numbered { seq, event });
        }
        self.wake.notify_all();
    }

    fn callback(&self, window: &str, body: &str) -> (u16, Value) {
        let (root, id) = {
            let state = self.state.lock().expect("daemon state");
            match state.windows.get(window) {
                Some(w) => (w.root.clone(), w.id.clone()),
                None => return (401, json!({"error": "no such window"})),
            }
        };
        let Ok(input) = serde_json::from_str::<Value>(body) else { return (400, json!({"error": "the body must be JSON"})) };
        let (Some(uuid), Some(kind)) = (input["uuid"].as_str().filter(|s| !s.is_empty()), input["type"].as_str().filter(|s| !s.is_empty())) else {
            self.trigger_log(input["uuid"].as_str().unwrap_or(""), input["type"].as_str().unwrap_or(""), &id, "rejected", "uuid and type are required");
            return (400, json!({"error": "a callback needs uuid and type"}));
        };
        if !self.state.lock().expect("daemon state").seen.insert(uuid.to_string()) {
            self.trigger_log(uuid, kind, &id, "duplicate", "");
            return (200, json!({"result": "duplicate"}));
        }
        let off = self.state.lock().expect("daemon state").windows.get(window).is_some_and(|w| w.off.contains(kind));
        if off {
            self.forget(uuid);
            self.trigger_log(uuid, kind, &id, "no_handler", "switched off");
            return (400, json!({"error": format!("the type {kind} is switched off in this project")}));
        }
        if let Some(handler) = self.custom_handler(window, kind) {
            return match self.run_handler(&id, handler, uuid, kind, input.clone()) {
                Ok(name) => {
                    self.trigger_log(uuid, kind, &id, "handled", &name);
                    (200, json!({"result": "handled", "handler": name}))
                }
                Err(message) => {
                    self.forget(uuid);
                    self.trigger_log(uuid, kind, &id, "failed", &message);
                    (400, json!({"error": message}))
                }
            };
        }
        let disabled_custom = self.handlers.iter().any(|h| h.types().iter().any(|t| t == kind));
        let handled = match kind {
            _ if disabled_custom => {
                self.forget(uuid);
                self.trigger_log(uuid, kind, &id, "no_handler", "switched off");
                return (400, json!({"error": format!("the type {kind} is switched off in this project")}));
            }
            "notification" => self.notification(&id, uuid, &input),
            "journal" => self.journal(&id, &root, &input),
            "flowstate_update" => self.flow(&id, &root, &input),
            _ if self.has_condition(&id, kind) => self.signal(&id, uuid, kind, &input),
            _ => {
                self.forget(uuid);
                self.trigger_log(uuid, kind, &id, "no_handler", "");
                return (400, json!({"error": format!("no handler for the type {kind}")}));
            }
        };
        match handled {
            Ok(result) => {
                self.trigger_log(uuid, kind, &id, "handled", "");
                (200, json!({"result": "handled", "handler": kind, "detail": result}))
            }
            Err(message) => {
                // A callback that failed may be sent again.
                self.forget(uuid);
                self.trigger_log(uuid, kind, &id, "failed", &message);
                (400, json!({"error": message}))
            }
        }
    }

    /// The custom handler for this type in this window: one chosen to override a standard type,
    /// or one serving the type and not switched off.
    fn custom_handler(&self, window: &str, kind: &str) -> Option<SharedHandler> {
        let state = self.state.lock().expect("daemon state");
        let project = &state.windows.get(window)?.project;
        if let Some(chosen) = project.handlers.get(kind)
            && let Some(handler) = self.handlers.iter().find(|h| h.name() == chosen)
        {
            return Some(handler.clone());
        }
        self.handlers.iter().find(|h| h.types().iter().any(|t| t == kind) && !project.disabled.iter().any(|d| d == h.name() || d == kind)).cloned()
    }

    fn run_handler(&self, window: &str, handler: SharedHandler, uuid: &str, kind: &str, input: Value) -> Result<String, String> {
        let callback = Callback { uuid: uuid.to_string(), kind: kind.to_string(), fields: input };
        let effect = pollster::block_on(handler.handle(&callback)).map_err(|e| format!("{}: {e}", handler.name()))?;
        if let HandlerEffect::Agent { label, text } = effect {
            self.deliver(window, WindowEvent::Notification { uuid: uuid.to_string(), label, text });
        }
        Ok(handler.name().to_string())
    }

    fn has_condition(&self, window: &str, kind: &str) -> bool {
        let state = self.state.lock().expect("daemon state");
        state.windows.get(window).is_some_and(|w| w.conditions.iter().any(|c| c.on == kind))
    }

    /// A signal accumulates and reaches no agent, unless a condition turns true: then one
    /// message for the whole batch (IF-15, INV-14). The payload is data (INV-12).
    fn signal(&self, window: &str, uuid: &str, kind: &str, input: &Value) -> Result<Value, String> {
        let callback = Callback { uuid: uuid.to_string(), kind: kind.to_string(), fields: input.clone() };
        let log_path = self.data_dir.join("daemon").join("triggers.jsonl");
        let fired: Vec<(String, String)> = {
            let mut state = self.state.lock().expect("daemon state");
            let w = state.windows.get_mut(window).ok_or("no such window")?;
            w.conditions.iter_mut().filter(|c| c.on == kind).filter_map(|c| c.signal(now_ms(), callback.clone(), &log_path)).collect()
        };
        let names: Vec<String> = fired.iter().map(|(name, _)| name.clone()).collect();
        for (label, text) in fired {
            self.deliver(window, WindowEvent::Notification { uuid: uuid.to_string(), label, text });
        }
        Ok(json!({"signal": true, "fired": names}))
    }

    fn forget(&self, uuid: &str) {
        self.state.lock().expect("daemon state").seen.remove(uuid);
    }

    /// INV-12: only the text and label travel; nothing in the payload selects anything.
    fn notification(&self, window: &str, uuid: &str, input: &Value) -> Result<Value, String> {
        // The channel's label is reserved: a callback is not the operator (INV-12).
        let label = input["label"].as_str().filter(|l| *l != TELEGRAM_LABEL && *l != harness_api::TELEGRAM_VOICE_LABEL && *l != harness_api::HARNESS_LABEL).unwrap_or("callback").to_string();
        let text = match input["text"].as_str() {
            Some(text) => text.to_string(),
            None => {
                let mut rest = input.clone();
                if let Some(map) = rest.as_object_mut() {
                    map.remove("uuid");
                    map.remove("type");
                    map.remove("label");
                }
                rest.to_string()
            }
        };
        self.deliver(window, WindowEvent::Notification { uuid: uuid.to_string(), label, text });
        Ok(json!({}))
    }

    fn memory(&self, root: &str) -> Result<JournalMemory, String> {
        self.memory_of(root, "daemon")
    }

    fn memory_of(&self, root: &str, owner: &str) -> Result<JournalMemory, String> {
        let key = key_for(git_origin(root).as_deref(), Path::new(root));
        JournalMemory::open(&self.data_dir.join("journals"), &key, owner)
    }

    /// CON-20: a session's Memory call, against the journal of the project it names.
    fn memory_op(&self, body: &str) -> (u16, Value) {
        let Ok(request) = serde_json::from_str::<memory::Request>(body) else {
            return (400, json!({"error": "not a memory request"}));
        };
        self.memory_ops.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let answer = self.memory_of(&request.root, &request.owner).and_then(|store| memory::serve(&store, request.call));
        match answer {
            Ok(result) => (200, json!({"result": result})),
            Err(error) => (500, json!({"error": error})),
        }
    }

    fn journal(&self, window: &str, root: &str, input: &Value) -> Result<Value, String> {
        let body = input["text"].as_str().or(input["body"].as_str()).ok_or("a journal callback needs text")?;
        let mut tags: Vec<String> = input["tags"].as_array().map(|t| t.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
        // An entry is found again by its tags, so one always has at least this.
        if tags.is_empty() {
            tags.push("callback".to_string());
        }
        let entry = JournalEntry {
            body: body.to_string(),
            category: input["category"].as_str().unwrap_or("finding").to_string(),
            tags,
            flags: vec![],
            salience: None,
            feature: None,
        };
        let memory = self.memory(root)?;
        let record = now_ready(memory.journal_add(entry)).map_err(|e| e.to_string())?;
        let id = record.id;
        self.deliver(window, WindowEvent::Journal { record });
        Ok(json!({"entry": id}))
    }

    /// ADR 0012: a pass is an instance of the reference chart, keyed by the client's pipeline id.
    /// An event naming a node the chart lacks is recorded as off-chart, never added to it.
    fn flow(&self, window: &str, root: &str, input: &Value) -> Result<Value, String> {
        let map_id = input["map"].as_u64().ok_or("a flowstate_update needs map")?;
        let node = input["node"].as_u64().ok_or("a flowstate_update needs node")?;
        let instance = input["instance"].as_str().or(input["correlation_id"].as_str()).ok_or("a flowstate_update needs instance")?.to_string();
        let state_name = input["state"].as_str().unwrap_or("entered").to_string();
        let memory = self.memory(root)?;
        let map = now_ready(memory.map_read(map_id)).map_err(|e| e.to_string())?.ok_or_else(|| format!("no map #{map_id}"))?;
        let off_chart = !map.nodes.iter().any(|n| n.id == node);
        let at_end = !off_chart && map.nodes.iter().any(|n| n.id == node && n.kind == "end");
        let step = stepdebug::Step { node, state: state_name.clone(), off_chart, at_ms: now_ms(), data: input["data"].clone() };
        let (path, finished) = {
            let mut state = self.state.lock().expect("daemon state");
            let w = state.windows.get_mut(window).ok_or("no such window")?;
            let key = (map_id, instance.clone());
            let path = w.instances.entry(key.clone()).or_default();
            if !off_chart {
                path.push(node);
            }
            let path = path.clone();
            let mut finished = None;
            if w.step_debug {
                w.cycles.entry(key.clone()).or_default().push(step);
                // The cycle ends here: the next event of this instance starts a new run.
                if at_end {
                    finished = w.cycles.remove(&key);
                    w.instances.remove(&key);
                }
            }
            (path, finished)
        };
        let run = match finished {
            Some(steps) => match stepdebug::save_run(root, map_id, &instance, &steps) {
                Ok(id) => Some(id),
                Err(e) => {
                    self.trigger_log("", "flowstate_update", window, "run_not_saved", &e);
                    None
                }
            },
            None => None,
        };
        self.deliver(window, WindowEvent::Flow { map, instance, node, state: state_name, off_chart, path, run });
        Ok(json!({"off_chart": off_chart, "run": run}))
    }
}

/// Windows are kept in a file, so callback URLs survive a daemon's restart.
fn save_windows(dir: &Path, windows: &HashMap<String, Window>) {
    let list: Vec<Value> = windows.values().map(|w| json!({"id": w.id, "root": w.root, "step_debug": w.step_debug})).collect();
    let _ = write_private(&dir.join("windows.json"), &Value::Array(list).to_string());
}

fn restore_windows(dir: &Path) -> HashMap<String, Window> {
    let text = std::fs::read_to_string(dir.join("windows.json")).unwrap_or_default();
    let list: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
    list.iter()
        .filter_map(|w| {
            let (id, root) = (w["id"].as_str()?.to_string(), w["root"].as_str()?.to_string());
            let window = Window {
                id: id.clone(),
                conditions: conditions::load(&root).unwrap_or_default(),
                off: off_types(&root),
                project: conditions::read_project(&root).unwrap_or_default(),
                root,
                events: Vec::new(),
                instances: HashMap::new(),
                step_debug: w["step_debug"] == true,
                cycles: HashMap::new(),
            };
            Some((id, window))
        })
        .collect()
}

/// The port a daemon last used here: a restart keeps the callback URLs the same.
pub fn last_port(data_dir: &Path) -> Option<u16> {
    let text = std::fs::read_to_string(address_path(data_dir)).ok()?;
    serde_json::from_str::<Address>(&text).ok().map(|a| a.port)
}

fn off_types(root: &str) -> HashSet<String> {
    let handlers = conditions::read_project(root).map(|p| p.handlers).unwrap_or_default();
    handlers.into_iter().filter(|(_, implementation)| implementation == "off").map(|(kind, _)| kind).collect()
}

/// The folder's `origin` URL, if it is a repository with one.
pub fn git_origin(root: &str) -> Option<String> {
    let out = std::process::Command::new("git").args(["-C", root, "config", "--get", "remote.origin.url"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|u| !u.is_empty())
}

/// The store's futures are ready at once; no executor is needed here.
fn now_ready<T>(mut future: harness_api::BoxFuture<'_, T>) -> T {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("the journal store answers at once"),
    }
}
