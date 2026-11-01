//! The daemon (CON-20): the single entry point of callbacks to agents, with a callback URL for
//! every open window (IF-13). A callback carries `uuid` and `type`; its type picks a handler
//! (IF-16). Its payload is data and cannot choose anything else (INV-12). The same uuid is
//! handled once (INV-14); a type with no handler reaches no one, is logged, and gets 400.

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use harness_api::{JournalEntry, JournalRecord, MapData, Memory};
use harness_memory_journal::plugin::JournalMemory;
use harness_memory_journal::project::key_for;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod client;

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
    Flow { map: MapData, instance: String, node: u64, state: String, off_chart: bool, path: Vec<u64> },
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
}

struct State {
    windows: HashMap<String, Window>,
    seen: HashSet<String>,
    next_seq: u64,
}

pub struct Daemon {
    state: Mutex<State>,
    wake: Condvar,
    admin: String,
    data_dir: PathBuf,
    log: Mutex<std::fs::File>,
    port: u16,
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
            state: Mutex::new(State { windows: HashMap::new(), seen, next_seq: 1 }),
            wake: Condvar::new(),
            admin,
            data_dir: data_dir.to_path_buf(),
            log: Mutex::new(log),
            port,
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

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn admin_token(&self) -> &str {
        &self.admin
    }

    fn serve(&self, mut request: tiny_http::Request) {
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
            ("GET", "/health") => (200, json!({"ok": true})),
            ("POST", "/register") if admin => self.register(&body),
            ("GET", "/events") if admin => self.events(query),
            ("POST", "/register") | ("GET", "/events") => (401, json!({"error": "bad token"})),
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
        let window = Window { id: id.clone(), root: root.to_string(), events: Vec::new(), instances: HashMap::new() };
        self.state.lock().expect("daemon state").windows.insert(id.clone(), window);
        (200, json!({"window": id, "url": format!("http://127.0.0.1:{}/callback/{id}", self.port)}))
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
            let Some(w) = state.windows.get(&window) else { return (404, json!({"error": "no such window"})) };
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
        let handled = match kind {
            "notification" => self.notification(&id, uuid, &input),
            "journal" => self.journal(&id, &root, &input),
            "flowstate_update" => self.flow(&id, &root, &input),
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

    fn forget(&self, uuid: &str) {
        self.state.lock().expect("daemon state").seen.remove(uuid);
    }

    /// INV-12: only the text and label travel; nothing in the payload selects anything.
    fn notification(&self, window: &str, uuid: &str, input: &Value) -> Result<Value, String> {
        let label = input["label"].as_str().unwrap_or("callback").to_string();
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
        let key = key_for(git_origin(root).as_deref(), Path::new(root));
        JournalMemory::open(&self.data_dir.join("journals"), &key, "daemon")
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
        let path = {
            let mut state = self.state.lock().expect("daemon state");
            let w = state.windows.get_mut(window).ok_or("no such window")?;
            let path = w.instances.entry((map_id, instance.clone())).or_default();
            if !off_chart {
                path.push(node);
            }
            path.clone()
        };
        self.deliver(window, WindowEvent::Flow { map, instance, node, state: state_name, off_chart, path });
        Ok(json!({"off_chart": off_chart}))
    }
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
