//! The daemon-centric conversation (ADR 0024): sessions run in the daemon, one consistent log
//! there, and a frontend is a client of it. `DaemonHost` is the daemon's side, `RemoteSessions` the
//! client's, `SessionLink` what a frontend holds: the same operations either way.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use harness_api::*;
use harness_core::RunEnd;
use harness_daemon::ConversationHost;
use harness_daemon::client::Client;
use serde_json::{Value, json};

use crate::{Factory, FromSession, Host, Sessions, Settings, TaskRefused};

/// A window that has not polled for this long is gone; its runs finish with nobody watching.
const WINDOW_GONE: Duration = Duration::from_secs(30);
const PUMP: Duration = Duration::from_millis(10);

fn end_to_json(end: &RunEnd) -> Value {
    match end {
        RunEnd::AwaitingInput => json!("awaiting"),
        RunEnd::Done => json!("done"),
        RunEnd::Abandoned => json!("abandoned"),
        RunEnd::Failed(message) => json!({"failed": message}),
    }
}

fn end_from_json(value: &Value) -> RunEnd {
    match value.as_str() {
        Some("done") => RunEnd::Done,
        Some("abandoned") => RunEnd::Abandoned,
        Some(_) => RunEnd::AwaitingInput,
        None => RunEnd::Failed(value["failed"].as_str().unwrap_or("failed").to_string()),
    }
}

fn to_wire(message: &FromSession) -> Value {
    match message {
        FromSession::Entry { tab, entry } => json!({"kind": "entry", "tab": tab, "line": harness_api::event::to_line(entry)}),
        FromSession::Ask { tab, call, capabilities } => json!({"kind": "ask", "tab": tab, "call": call, "capabilities": capabilities}),
        FromSession::Ended { tab, end } => json!({"kind": "ended", "tab": tab, "end": end_to_json(end)}),
        FromSession::Unavailable { tab, message } => json!({"kind": "unavailable", "tab": tab, "message": message}),
    }
}

fn from_wire(value: &Value) -> Option<FromSession> {
    let tab = value["tab"].as_u64()?;
    match value["kind"].as_str()? {
        "entry" => Some(FromSession::Entry { tab, entry: harness_api::event::from_line(value["line"].as_str()?).ok()? }),
        "ask" => Some(FromSession::Ask { tab, call: serde_json::from_value(value["call"].clone()).ok()?, capabilities: serde_json::from_value(value["capabilities"].clone()).ok()? }),
        "ended" => Some(FromSession::Ended { tab, end: end_from_json(&value["end"]) }),
        "unavailable" => Some(FromSession::Unavailable { tab, message: value["message"].as_str()?.to_string() }),
        _ => None,
    }
}

fn answer_to_json(answer: Answer) -> &'static str {
    match answer {
        Answer::AllowOnce => "once",
        Answer::AlwaysAllow => "always",
        Answer::Deny => "deny",
    }
}

/// Builds what a session gets from this binary, in the daemon as in a frontend.
pub type MakeHost = Arc<dyn Fn(&Settings, Arc<dyn Fn(u64) -> Option<String> + Send + Sync>, Arc<AtomicBool>) -> Host + Send + Sync>;

struct Window {
    sessions: Sessions,
    root: String,
    events: VecDeque<(u64, Value)>,
    next: u64,
    reload: Arc<AtomicBool>,
    notes: Arc<Mutex<HashMap<u64, String>>>,
    polled: Instant,
    closed: bool,
    /// Tabs whose prompt nobody has answered yet.
    asking: HashSet<u64>,
}

/// The daemon's side: every conversation of every window runs here, and its log is written here.
pub struct DaemonHost {
    windows: Mutex<HashMap<String, Window>>,
    wake: Condvar,
    data_dir: PathBuf,
    make_host: MakeHost,
}

impl DaemonHost {
    pub fn new(data_dir: &Path, make_host: MakeHost) -> Arc<DaemonHost> {
        let host = Arc::new(DaemonHost { windows: Mutex::new(HashMap::new()), wake: Condvar::new(), data_dir: data_dir.to_path_buf(), make_host });
        let pump = host.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(PUMP);
                pump.pump();
            }
        });
        host
    }

    /// Takes what the sessions said and queues it for their windows.
    fn pump(&self) {
        let mut windows = self.windows.lock().expect("windows");
        let mut any = false;
        for window in windows.values_mut() {
            let messages = window.sessions.drain();
            let reload = window.reload.swap(false, Ordering::SeqCst);
            if window.polled.elapsed() > WINDOW_GONE {
                window.closed = true;
            }
            for message in &messages {
                if let FromSession::Ask { tab, .. } = message {
                    window.asking.insert(*tab);
                }
            }
            if window.closed {
                // Nobody is there to answer: a prompt is a denial, and the run goes on to its end.
                for tab in window.asking.drain() {
                    window.sessions.answer(tab, Answer::Deny);
                }
                window.events.clear();
                continue;
            }
            for message in messages {
                window.next += 1;
                window.events.push_back((window.next, to_wire(&message)));
                any = true;
            }
            if reload {
                window.next += 1;
                window.events.push_back((window.next, json!({"kind": "reload"})));
                any = true;
            }
        }
        // A closed window stays until its runs have finished: the log gets their end.
        windows.retain(|_, w| !w.closed || w.sessions.busy());
        if any {
            self.wake.notify_all();
        }
    }

    fn open(&self, windows: &mut HashMap<String, Window>, id: &str, op: &Value) -> Result<Value, String> {
        let root = op["root"].as_str().ok_or("root is required")?.to_string();
        let settings: Settings = serde_json::from_value(op["settings"].clone()).map_err(|e| format!("settings: {e}"))?;
        let (reload, notes) = match windows.get(id) {
            Some(w) => (w.reload.clone(), w.notes.clone()),
            None => (Arc::default(), Arc::default()),
        };
        let lookup = notes.clone();
        let note: Arc<dyn Fn(u64) -> Option<String> + Send + Sync> = Arc::new(move |tab| lookup.lock().ok()?.get(&tab).cloned());
        let host = (self.make_host)(&settings, note, reload.clone());
        let factory = crate::with_host(settings, root.clone(), self.data_dir.clone(), host);
        match windows.get_mut(id) {
            Some(window) => window.sessions.set_factory(factory),
            None => {
                let sessions = Sessions::new(factory, root.clone()).with_data_dir(&self.data_dir.to_string_lossy());
                windows.insert(id.to_string(), Window { sessions, root, events: VecDeque::new(), next: 0, reload, notes, polled: Instant::now(), closed: false, asking: HashSet::new() });
            }
        }
        Ok(json!({"ok": true, "pid": std::process::id()}))
    }
}

impl ConversationHost for DaemonHost {
    fn op(&self, id: &str, op: &Value) -> Result<Value, String> {
        let mut windows = self.windows.lock().expect("windows");
        let name = op["op"].as_str().ok_or("op is required")?;
        if name == "open" {
            return self.open(&mut windows, id, op);
        }
        if name == "busy" {
            return Ok(json!({"busy": windows.values().any(|w| w.sessions.busy())}));
        }
        // One writer to a log: a run left by a closed window finishes before the session opens again.
        if name == "resume" && windows.iter().any(|(other, w)| other != id && op["text"].as_str().is_some_and(|session| w.sessions.runs(session))) {
            let window = windows.get_mut(id).ok_or("this window is not open here")?;
            window.next += 1;
            let tab = op["tab"].as_u64().unwrap_or(0);
            window.events.push_back((window.next, to_wire(&FromSession::Unavailable { tab, message: "This conversation is still finishing a run in the daemon. Open it again in a moment: all of it will be there.".to_string() })));
            self.wake.notify_all();
            return Ok(json!({"ok": false}));
        }
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        window.polled = Instant::now();
        let tab = op["tab"].as_u64().unwrap_or(0);
        let text = || op["text"].as_str().unwrap_or_default().to_string();
        let on = op["on"].as_bool().unwrap_or(false);
        match name {
            "prompt" => window.sessions.prompt(tab, text()),
            "task" => {
                let steps = serde_json::from_value(op["steps"].clone()).map_err(|e| e.to_string())?;
                return Ok(match window.sessions.task(tab, text(), steps) {
                    Ok(()) => json!({"ok": true}),
                    Err(TaskRefused::Invalid(reason)) => json!({"invalid": reason}),
                    Err(TaskRefused::NotApproved { commands, text }) => json!({"not_approved": {"commands": commands, "text": text}}),
                });
            }
            "nick" => window.sessions.nick(tab, text()),
            "step_debug" => window.sessions.step_debug(tab, on),
            "plan" => window.sessions.plan(tab, on),
            "inbound" => window.sessions.inbound(tab, op["label"].as_str().unwrap_or_default().to_string(), text()),
            "answer" => {
                window.asking.remove(&tab);
                window.sessions.answer(tab, match op["answer"].as_str() {
                    Some("once") => Answer::AllowOnce,
                    Some("always") => Answer::AlwaysAllow,
                    _ => Answer::Deny,
                });
            }
            "stop" => window.sessions.stop(tab),
            "resume" => window.sessions.resume(tab, &text()),
            "rebind" => return Ok(json!({"ok": window.sessions.rebind(tab)})),
            "unqueue" => return Ok(json!({"ok": window.sessions.unqueue(tab, &text())})),
            "session_of" => return Ok(json!({"session": window.sessions.session_of(tab)})),
            "note" => {
                window.notes.lock().expect("notes").insert(tab, text());
            }
            "close" => window.closed = true,
            other => return Err(format!("no such operation: {other}")),
        }
        Ok(json!({"ok": true}))
    }

    fn events(&self, id: &str, after: u64, wait: Duration) -> Vec<Value> {
        let deadline = Instant::now() + wait;
        let mut windows = self.windows.lock().expect("windows");
        loop {
            let Some(window) = windows.get_mut(id) else { return Vec::new() };
            window.polled = Instant::now();
            while window.events.front().is_some_and(|(seq, _)| *seq <= after) {
                window.events.pop_front();
            }
            if !window.events.is_empty() {
                return window.events.iter().map(|(seq, event)| json!({"seq": seq, "event": event})).collect();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Vec::new();
            }
            windows = self.wake.wait_timeout(windows, left).expect("windows").0;
        }
    }

    fn stats(&self) -> Value {
        let windows = self.windows.lock().expect("windows");
        json!({"windows": windows.len(), "busy": windows.values().filter(|w| w.sessions.busy()).count(), "roots": windows.values().map(|w| w.root.clone()).collect::<HashSet<_>>()})
    }
}

/// The client's side: the same operations as `Sessions`, all of them answered by the daemon.
pub struct RemoteSessions {
    client: Client,
    window: String,
    incoming: Receiver<FromSession>,
    tabs: Arc<Mutex<HashSet<u64>>>,
    stopped: Arc<AtomicBool>,
}

impl RemoteSessions {
    /// Opens this window in the daemon and starts listening. `reload` is raised when a session
    /// asks for a hot reload.
    pub fn open(client: Client, root: &str, settings: &Settings, reload: Arc<AtomicBool>) -> Result<RemoteSessions, String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let window = format!("conv-{}-{now}", std::process::id());
        client.conversation_op(&json!({"window": window, "op": "open", "root": root, "settings": settings}))?;
        let (sender, incoming) = channel();
        let (tabs, stopped): (Arc<Mutex<HashSet<u64>>>, Arc<AtomicBool>) = (Arc::default(), Arc::default());
        listen(client.clone(), window.clone(), sender, tabs.clone(), stopped.clone(), reload);
        Ok(RemoteSessions { client, window, incoming, tabs, stopped })
    }

    fn call(&self, op: &str, tab: u64, mut rest: Value) -> Option<Value> {
        rest["window"] = json!(self.window);
        rest["op"] = json!(op);
        rest["tab"] = json!(tab);
        self.tabs.lock().expect("tabs").insert(tab);
        // The listener reports a daemon that went away.
        self.client.conversation_op(&rest).ok()
    }

    pub fn configure(&self, root: &str, settings: &Settings) -> Result<(), String> {
        self.client.conversation_op(&json!({"window": self.window, "op": "open", "root": root, "settings": settings})).map(|_| ())
    }
}

impl Drop for RemoteSessions {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.client.conversation_op(&json!({"window": self.window, "op": "close"}));
    }
}

fn listen(client: Client, window: String, sender: Sender<FromSession>, tabs: Arc<Mutex<HashSet<u64>>>, stopped: Arc<AtomicBool>, reload: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let (mut after, mut failures) = (0, 0);
        while !stopped.load(Ordering::SeqCst) {
            match client.conversation_events(&window, after) {
                Ok(events) => {
                    failures = 0;
                    for event in events {
                        after = event["seq"].as_u64().unwrap_or(after);
                        if event["event"]["kind"] == "reload" {
                            reload.store(true, Ordering::SeqCst);
                        } else if let Some(message) = from_wire(&event["event"]) && sender.send(message).is_err() {
                            return;
                        }
                    }
                }
                Err(_) => {
                    failures += 1;
                    if failures == 5 {
                        for tab in tabs.lock().expect("tabs").iter() {
                            let _ = sender.send(FromSession::Unavailable { tab: *tab, message: "The daemon that runs this conversation stopped. The log keeps it: open it again from the start page.".to_string() });
                        }
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    });
}

/// What a frontend holds. `Local` is for scripted runs and for when no daemon can be had.
pub enum SessionLink {
    Local(Sessions),
    Remote(RemoteSessions),
}

impl SessionLink {
    pub fn is_remote(&self) -> bool {
        matches!(self, SessionLink::Remote(_))
    }

    pub fn configure(&mut self, root: &str, settings: &Settings, local: impl FnOnce() -> Factory) -> Result<(), String> {
        match self {
            SessionLink::Local(s) => {
                s.set_factory(local());
                Ok(())
            }
            SessionLink::Remote(r) => r.configure(root, settings),
        }
    }

    pub fn note(&self, tab: u64, text: String) {
        if let SessionLink::Remote(r) = self {
            r.call("note", tab, json!({"text": text}));
        }
    }

    pub fn resume(&mut self, tab: u64, session: &str) {
        match self {
            SessionLink::Local(s) => s.resume(tab, session),
            SessionLink::Remote(r) => {
                r.call("resume", tab, json!({"text": session}));
            }
        }
    }

    pub fn session_of(&self, tab: u64) -> Option<String> {
        match self {
            SessionLink::Local(s) => s.session_of(tab).map(str::to_string),
            SessionLink::Remote(r) => r.call("session_of", tab, json!({}))?["session"].as_str().map(str::to_string),
        }
    }

    pub fn inbound(&mut self, tab: u64, label: String, text: String) {
        match self {
            SessionLink::Local(s) => s.inbound(tab, label, text),
            SessionLink::Remote(r) => {
                r.call("inbound", tab, json!({"label": label, "text": text}));
            }
        }
    }

    pub fn rebind(&mut self, tab: u64) -> bool {
        match self {
            SessionLink::Local(s) => s.rebind(tab),
            SessionLink::Remote(r) => r.call("rebind", tab, json!({})).is_none_or(|reply| reply["ok"] == true),
        }
    }

    pub fn prompt(&mut self, tab: u64, text: String) {
        match self {
            SessionLink::Local(s) => s.prompt(tab, text),
            SessionLink::Remote(r) => {
                r.call("prompt", tab, json!({"text": text}));
            }
        }
    }

    pub fn task(&mut self, tab: u64, text: String, steps: Vec<String>) -> Result<(), TaskRefused> {
        match self {
            SessionLink::Local(s) => s.task(tab, text, steps),
            SessionLink::Remote(r) => {
                let reply = r.call("task", tab, json!({"text": text, "steps": steps})).ok_or_else(|| TaskRefused::Invalid("the daemon did not answer".to_string()))?;
                if let Some(reason) = reply["invalid"].as_str() {
                    return Err(TaskRefused::Invalid(reason.to_string()));
                }
                if reply["not_approved"].is_object() {
                    let commands = serde_json::from_value(reply["not_approved"]["commands"].clone()).unwrap_or_default();
                    return Err(TaskRefused::NotApproved { commands, text: reply["not_approved"]["text"].as_str().unwrap_or_default().to_string() });
                }
                Ok(())
            }
        }
    }

    pub fn plan(&mut self, tab: u64, on: bool) {
        match self {
            SessionLink::Local(s) => s.plan(tab, on),
            SessionLink::Remote(r) => {
                r.call("plan", tab, json!({"on": on}));
            }
        }
    }

    pub fn step_debug(&mut self, tab: u64, on: bool) {
        match self {
            SessionLink::Local(s) => s.step_debug(tab, on),
            SessionLink::Remote(r) => {
                r.call("step_debug", tab, json!({"on": on}));
            }
        }
    }

    pub fn nick(&mut self, tab: u64, name: String) {
        match self {
            SessionLink::Local(s) => s.nick(tab, name),
            SessionLink::Remote(r) => {
                r.call("nick", tab, json!({"text": name}));
            }
        }
    }

    pub fn unqueue(&mut self, tab: u64, text: &str) -> bool {
        match self {
            SessionLink::Local(s) => s.unqueue(tab, text),
            SessionLink::Remote(r) => r.call("unqueue", tab, json!({"text": text})).is_some_and(|reply| reply["ok"] == true),
        }
    }

    pub fn answer(&self, tab: u64, answer: Answer) {
        match self {
            SessionLink::Local(s) => s.answer(tab, answer),
            SessionLink::Remote(r) => {
                r.call("answer", tab, json!({"answer": answer_to_json(answer)}));
            }
        }
    }

    pub fn stop(&self, tab: u64) {
        match self {
            SessionLink::Local(s) => s.stop(tab),
            SessionLink::Remote(r) => {
                r.call("stop", tab, json!({}));
            }
        }
    }

    pub fn drain(&self) -> Vec<FromSession> {
        match self {
            SessionLink::Local(s) => s.drain(),
            SessionLink::Remote(r) => r.incoming.try_iter().collect(),
        }
    }
}

/// The daemon for this binary: found or started; one of another build is replaced when idle.
pub fn daemon_for_this_build(data_dir: &Path, program: &Path) -> Result<Client, String> {
    let client = Client::find_or_start(data_dir, program)?;
    if client.build().as_deref() == Some(&harness_daemon::build_id()) {
        return Ok(client);
    }
    let busy = client.conversation_op(&json!({"window": "-", "op": "busy"})).is_ok_and(|reply| reply["busy"] == true);
    if busy {
        return Err("a daemon of another build is running conversations".to_string());
    }
    client.shutdown()?;
    Client::find_or_start(data_dir, program)
}
