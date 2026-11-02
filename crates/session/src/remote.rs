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
/// How long a closing daemon waits for its sessions' threads. Idle ones end at once.
const CLOSING: Duration = Duration::from_secs(5);
/// A window polls every couple of seconds. One silent for this long is not there, as far as
/// opening its conversation elsewhere goes.
const WINDOW_SILENT: Duration = Duration::from_secs(5);
/// How long a window waits for one that may have just gone, before it says the other is open.
const WAIT_FOR_OTHER: Duration = Duration::from_secs(8);

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
        FromSession::Synced { tab, error } => json!({"kind": "synced", "tab": tab, "error": error}),
    }
}

fn from_wire(value: &Value) -> Option<FromSession> {
    let tab = value["tab"].as_u64()?;
    match value["kind"].as_str()? {
        "entry" => Some(FromSession::Entry { tab, entry: harness_api::event::from_line(value["line"].as_str()?).ok()? }),
        "ask" => Some(FromSession::Ask { tab, call: serde_json::from_value(value["call"].clone()).ok()?, capabilities: serde_json::from_value(value["capabilities"].clone()).ok()? }),
        "ended" => Some(FromSession::Ended { tab, end: end_from_json(&value["end"]) }),
        "unavailable" => Some(FromSession::Unavailable { tab, message: value["message"].as_str()?.to_string() }),
        "synced" => Some(FromSession::Synced { tab, error: value["error"].as_str().map(str::to_string) }),
        _ => None,
    }
}

fn tab_of(message: &FromSession) -> u64 {
    match message {
        FromSession::Entry { tab, .. } | FromSession::Ask { tab, .. } | FromSession::Ended { tab, .. } | FromSession::Unavailable { tab, .. } | FromSession::Synced { tab, .. } => *tab,
    }
}

fn answer_to_json(answer: Answer) -> &'static str {
    match answer {
        Answer::AllowOnce => "once",
        Answer::AlwaysAllow => "always",
        Answer::Deny => "deny",
    }
}

fn answer_from_json(value: &Value) -> Answer {
    match value.as_str() {
        Some("once") => Answer::AllowOnce,
        Some("always") => Answer::AlwaysAllow,
        _ => Answer::Deny,
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
    /// What each tab runs on, once it is settled.
    choices: crate::Choices,
    /// What a conversation with no choice of its own starts on.
    default: crate::Choice,
    polled: Instant,
    closed: bool,
    /// Tabs whose prompt nobody has answered yet, with the prompt.
    asking: HashMap<u64, Value>,
    /// Runs of this window that another window has taken over: tab -> (window, its tab).
    handed: HashMap<u64, (String, u64)>,
    /// This window's tabs showing a run that another window started.
    following: HashMap<u64, Follow>,
    /// What each tab's run has said so far: the log has it only as of its last flush, and a window
    /// taking the run over needs all of it.
    said: HashMap<u64, Vec<Value>>,
    stopped_starting: bool,
    /// The window's environment, for reading a provider's conversations (the daemon's own is not it).
    environment: Option<crate::WindowEnv>,
}

/// A tab showing a run of another window. When it ends the tab has the session itself.
struct Follow {
    window: String,
    tab: u64,
    session: String,
    /// What was asked of the tab meanwhile, in order.
    pending: Vec<Value>,
}

impl Window {
    fn silent(&self) -> bool {
        self.closed || self.polled.elapsed() > WINDOW_SILENT
    }

    fn say(&mut self, event: Value) {
        self.next += 1;
        self.events.push_back((self.next, event));
    }

    /// One operation on this window's own sessions.
    fn apply(&mut self, name: &str, op: &Value) -> Result<Value, String> {
        let tab = op["tab"].as_u64().unwrap_or(0);
        let text = || op["text"].as_str().unwrap_or_default().to_string();
        let on = op["on"].as_bool().unwrap_or(false);
        match name {
            "prompt" => self.sessions.prompt(tab, text()),
            "task" => {
                let steps = serde_json::from_value(op["steps"].clone()).map_err(|e| e.to_string())?;
                return Ok(match self.sessions.task(tab, text(), steps) {
                    Ok(()) => json!({"ok": true}),
                    Err(TaskRefused::Invalid(reason)) => json!({"invalid": reason}),
                    Err(TaskRefused::NotApproved { commands, text }) => json!({"not_approved": {"commands": commands, "text": text}}),
                });
            }
            "approve" => {
                self.sessions.approve_acceptance(&text())?;
            }
            "nick" => self.sessions.nick(tab, text()),
            "step_debug" => self.sessions.step_debug(tab, on),
            "plan" => self.sessions.plan(tab, on),
            "inbound" => self.sessions.inbound(tab, op["label"].as_str().unwrap_or_default().to_string(), text()),
            "answer" => {
                self.asking.remove(&tab);
                self.sessions.answer(tab, answer_from_json(&op["answer"]));
            }
            "stop" => self.sessions.stop(tab),
            "resume" => self.sessions.resume(tab, &text()),
            "rebind" => return Ok(json!({"ok": self.sessions.rebind(tab)})),
            "unqueue" => return Ok(json!({"ok": self.sessions.unqueue(tab, &text())})),
            "session_of" => return Ok(json!({"session": self.sessions.session_of(tab)})),
            "note" => {
                self.notes.lock().expect("notes").insert(tab, text());
            }
            "close" => self.closed = true,
            other => return Err(format!("no such operation: {other}")),
        }
        Ok(json!({"ok": true}))
    }
}

/// The daemon's side: every conversation of every window runs here, and its log is written here.
pub struct DaemonHost {
    windows: Mutex<HashMap<String, Window>>,
    /// Messages a window sent and went away from, by session: for whoever opens it next. Only in
    /// memory: a message that never reached the log is not the conversation yet.
    left: Mutex<HashMap<String, Vec<String>>>,
    /// The names sessions go by, by session: a log can be far too large to read them from.
    nicks: Mutex<HashMap<String, String>>,
    /// What each conversation runs on, by session (CON-94). Kept, so a reload or a change of the
    /// default moves nothing that is already going.
    conversations: Mutex<HashMap<String, crate::Choice>>,
    wake: Condvar,
    data_dir: PathBuf,
    make_host: MakeHost,
}

fn nicks_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("nicks.json")
}

fn conversations_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("conversations.json")
}

fn read_conversations(data_dir: &Path) -> HashMap<String, crate::Choice> {
    std::fs::read_to_string(conversations_path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn read_nicks(data_dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(nicks_path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

impl DaemonHost {
    /// A tab has this session: from now on it runs on what the conversation runs on, what it was
    /// told to, or else what is the default now.
    fn settle(&self, window: &Window, tab: u64, session: &str) {
        let mut stored = self.conversations.lock().expect("conversations");
        let mut choices = window.choices.lock().expect("choices");
        if let Some(choice) = stored.get(session) {
            choices.insert(tab, choice.clone());
            return;
        }
        let choice = choices.get(&tab).cloned().unwrap_or_else(|| window.default.clone());
        choices.insert(tab, choice.clone());
        stored.insert(session.to_string(), choice);
        self.save_conversations(&stored);
    }

    fn save_conversations(&self, stored: &HashMap<String, crate::Choice>) {
        let path = conversations_path(&self.data_dir);
        let partial = path.with_extension("partial");
        if serde_json::to_string(stored).is_ok_and(|text| std::fs::write(&partial, text).is_ok()) {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    /// The user chooses for this tab's conversation, and it is brought in line. No other moves.
    fn choose(&self, window: &mut Window, tab: u64, choice: crate::Choice) -> Result<Value, String> {
        if window.sessions.running(tab) || window.asking.contains_key(&tab) {
            return Err("a run is on in this conversation: its adapter can be changed when it ends".to_string());
        }
        window.choices.lock().expect("choices").insert(tab, choice.clone());
        if let Some(session) = window.sessions.session_of(tab) {
            let mut stored = self.conversations.lock().expect("conversations");
            stored.insert(session.to_string(), choice);
            self.save_conversations(&stored);
        }
        Ok(json!({"ok": window.sessions.rebind(tab)}))
    }

    fn remember_nick(&self, session: &str, nick: &str) {
        let mut nicks = self.nicks.lock().expect("nicks");
        if nicks.get(session).map(String::as_str) == Some(nick) {
            return;
        }
        nicks.insert(session.to_string(), nick.to_string());
        let path = nicks_path(&self.data_dir);
        let partial = path.with_extension("partial");
        if serde_json::to_string(&*nicks).is_ok_and(|text| std::fs::write(&partial, text).is_ok()) {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    pub fn new(data_dir: &Path, make_host: MakeHost) -> Arc<DaemonHost> {
        let host = Arc::new(DaemonHost { windows: Mutex::new(HashMap::new()), left: Mutex::new(HashMap::new()), nicks: Mutex::new(read_nicks(data_dir)), conversations: Mutex::new(read_conversations(data_dir)), wake: Condvar::new(), data_dir: data_dir.to_path_buf(), make_host });
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
        // A handed-over run speaks to the window that took it.
        let mut handed = Vec::new();
        for (id, window) in windows.iter_mut() {
            let messages = window.sessions.drain();
            let reload = window.reload.swap(false, Ordering::SeqCst);
            if window.polled.elapsed() > WINDOW_GONE {
                window.closed = true;
            }
            // Gone: what it was still going to send does not start by itself. Its sender may be
            // on the conversation in another program by now (INV-20).
            if window.silent() && !window.stopped_starting {
                window.stopped_starting = true;
                let mut left = self.left.lock().expect("left");
                for (session, waiting) in window.sessions.stop_starting() {
                    left.entry(session).or_default().extend(waiting);
                }
            }
            let (theirs, own): (Vec<_>, Vec<_>) = messages.into_iter().partition(|m| window.handed.contains_key(&tab_of(m)));
            for message in theirs {
                let (to, tab) = window.handed[&tab_of(&message)].clone();
                handed.push((id.clone(), tab_of(&message), to, tab, message));
            }
            for message in &own {
                match message {
                    FromSession::Ask { tab, .. } => {
                        window.asking.insert(*tab, to_wire(message));
                    }
                    FromSession::Entry { tab, entry } => {
                        if let harness_api::Event::AgentName { name, .. } = &entry.event
                            && let Some(session) = window.sessions.session_of(*tab)
                        {
                            self.remember_nick(session, name);
                        }
                        window.said.entry(*tab).or_default().push(to_wire(message));
                    }
                    FromSession::Ended { tab, .. } => {
                        window.said.remove(tab);
                    }
                    FromSession::Unavailable { .. } | FromSession::Synced { .. } => {}
                }
            }
            if window.closed {
                window.events.clear();
                continue;
            }
            for message in own {
                window.say(to_wire(&message));
                any = true;
            }
            if reload {
                window.say(json!({"kind": "reload"}));
                any = true;
            }
        }
        for (from, from_tab, to, tab, message) in handed {
            any |= Self::bring(&mut windows, &from, from_tab, &to, tab, message);
        }
        // Nobody is there to answer: a prompt is a denial, and the run goes on to its end.
        let unanswered: Vec<(String, u64)> = windows.iter_mut().filter(|(_, w)| w.closed).flat_map(|(id, w)| w.asking.drain().map(|(tab, _)| (id.clone(), tab)).collect::<Vec<_>>()).collect();
        for (id, tab) in unanswered {
            let source = windows.get(&id).and_then(|w| w.following.get(&tab)).map(|f| (f.window.clone(), f.tab));
            let (owner, owner_tab) = source.unwrap_or((id, tab));
            if let Some(window) = windows.get(&owner) {
                window.sessions.answer(owner_tab, Answer::Deny);
            }
        }
        // A closed window stays until its runs have finished: the log gets their end.
        windows.retain(|_, w| !w.closed || w.sessions.busy() || !w.handed.is_empty() || !w.following.is_empty());
        if any {
            self.wake.notify_all();
        }
    }

    /// One message of a handed-over run reaches the tab following it. When the run has ended, the
    /// session is that tab's own, and what was asked of it meanwhile goes in.
    fn bring(windows: &mut HashMap<String, Window>, from: &str, from_tab: u64, to: &str, tab: u64, message: FromSession) -> bool {
        let ended = matches!(message, FromSession::Ended { .. });
        let retabbed = match message {
            FromSession::Entry { entry, .. } => FromSession::Entry { tab, entry },
            FromSession::Ask { call, capabilities, .. } => FromSession::Ask { tab, call, capabilities },
            FromSession::Ended { end, .. } => FromSession::Ended { tab, end },
            FromSession::Unavailable { message, .. } => FromSession::Unavailable { tab, message },
            FromSession::Synced { error, .. } => FromSession::Synced { tab, error },
        };
        let mut said = false;
        if let (FromSession::Entry { .. }, Some(source)) = (&retabbed, windows.get_mut(from)) {
            source.said.entry(from_tab).or_default().push(to_wire(&retabbed));
        }
        if let Some(window) = windows.get_mut(to) {
            if let FromSession::Ask { .. } = retabbed {
                window.asking.insert(tab, to_wire(&retabbed));
            }
            if !window.closed {
                window.say(to_wire(&retabbed));
                said = true;
            }
        }
        if !ended {
            return said;
        }
        if let Some(source) = windows.get_mut(from) {
            source.handed.remove(&from_tab);
            source.sessions.forget(from_tab);
        }
        if let Some(window) = windows.get_mut(to)
            && let Some(follow) = window.following.remove(&tab)
        {
            window.sessions.resume(tab, &follow.session);
            for op in follow.pending {
                let _ = window.apply(op["op"].as_str().unwrap_or_default(), &op);
            }
        }
        said
    }

    /// Opening a session some other window holds. One writer to a log (CON-77): a window still
    /// there keeps it; from one that went away, the session comes here, with its run if it is on.
    fn take_over(windows: &mut HashMap<String, Window>, id: &str, tab: u64, session: &str) -> Option<Value> {
        let root = windows.get(id)?.root.clone();
        let (other, other_tab) = windows.iter().filter(|(other, w)| other.as_str() != id && w.root == root).find_map(|(other, w)| Some((other.clone(), w.sessions.tab_of(session)?)))?;
        let source = &windows[&other];
        // A process has one window: an older one of this process is what a hot reload left.
        let this_process = |window: &str| window.split('-').nth(1).map(str::to_string);
        let left_behind = this_process(&other).is_some() && this_process(&other) == this_process(id);
        let shown_by = source.handed.get(&other_tab).map(|(window, _)| window.clone());
        let there = match &shown_by {
            Some(window) if window == id => false,
            Some(window) => windows.get(window).is_some_and(|w| !w.silent()),
            None => !source.silent() && !left_behind,
        };
        if there {
            return Some(json!({"elsewhere": true}));
        }
        let running = source.sessions.runs(session);
        if let Some(window) = shown_by.as_ref().and_then(|window| windows.get_mut(window)) {
            window.following.retain(|_, f| f.session != session);
        }
        let source = windows.get_mut(&other)?;
        source.closed = true;
        if !running {
            source.handed.remove(&other_tab);
            source.sessions.forget(other_tab);
            return None;
        }
        let waiting = source.sessions.hand_over(other_tab);
        let said = source.said.get(&other_tab).cloned().unwrap_or_default();
        let prompt = source.asking.remove(&other_tab);
        source.handed.insert(other_tab, (id.to_string(), tab));
        let window = windows.get_mut(id)?;
        window.sessions.forget(tab);
        let pending = waiting.into_iter().map(|text| json!({"op": "prompt", "tab": tab, "text": text})).collect();
        window.following.insert(tab, Follow { window: other, tab: other_tab, session: session.to_string(), pending });
        // First what was said so far: the log may not have the turn in progress yet.
        for mut entry in said {
            entry["tab"] = json!(tab);
            window.say(entry);
        }
        if let Some(mut prompt) = prompt {
            prompt["tab"] = json!(tab);
            window.asking.insert(tab, prompt.clone());
            window.say(prompt);
        }
        Some(json!({"ok": true, "attached": true}))
    }

    /// An operation on a tab showing another window's run.
    fn on_followed(windows: &mut HashMap<String, Window>, id: &str, tab: u64, name: &str, op: &Value) -> Result<Value, String> {
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        let follow = window.following.get_mut(&tab).ok_or("not following")?;
        match name {
            "session_of" => Ok(json!({"session": follow.session})),
            "rebind" => Ok(json!({"ok": false})),
            "task" => Ok(json!({"invalid": "this conversation is finishing a run; give the task when it has ended"})),
            "unqueue" => {
                let at = follow.pending.iter().position(|p| p["op"] == "prompt" && p["text"] == op["text"]);
                if let Some(at) = at {
                    follow.pending.remove(at);
                }
                Ok(json!({"ok": at.is_some()}))
            }
            "answer" | "stop" => {
                let (owner, owner_tab) = (follow.window.clone(), follow.tab);
                window.asking.remove(&tab);
                let owner = windows.get(&owner).ok_or("the run has ended")?;
                if name == "stop" {
                    owner.sessions.stop(owner_tab);
                } else {
                    owner.sessions.answer(owner_tab, answer_from_json(&op["answer"]));
                }
                Ok(json!({"ok": true}))
            }
            "note" | "close" => window.apply(name, op),
            _ => {
                follow.pending.push(op.clone());
                Ok(json!({"ok": true}))
            }
        }
    }

    fn open(&self, windows: &mut HashMap<String, Window>, id: &str, op: &Value) -> Result<Value, String> {
        let root = op["root"].as_str().ok_or("root is required")?.to_string();
        let settings: Settings = serde_json::from_value(op["settings"].clone()).map_err(|e| format!("settings: {e}"))?;
        let default = crate::Choice { provider: settings.provider.clone(), model: settings.model.clone() };
        let (reload, notes, choices) = match windows.get(id) {
            Some(w) => (w.reload.clone(), w.notes.clone(), w.choices.clone()),
            None => (Arc::default(), Arc::default(), Arc::default()),
        };
        let lookup = notes.clone();
        let note: Arc<dyn Fn(u64) -> Option<String> + Send + Sync> = Arc::new(move |tab| lookup.lock().ok()?.get(&tab).cloned());
        let environment: Option<crate::WindowEnv> = serde_json::from_value(op["env"].clone()).ok().map(Arc::new);
        let mut host = (self.make_host)(&settings, note, reload.clone());
        host.environment = environment.clone();
        host.window = Some(id.to_string());
        host.choices = choices.clone();
        let factory = crate::with_host(settings, root.clone(), self.data_dir.clone(), host);
        match windows.get_mut(id) {
            Some(window) => window.sessions.set_factory(factory),
            None => {
                let sessions = Sessions::new(factory, root.clone()).with_data_dir(&self.data_dir.to_string_lossy());
                windows.insert(id.to_string(), Window { sessions, root, events: VecDeque::new(), next: 0, reload, notes, choices, default: default.clone(), polled: Instant::now(), closed: false, asking: HashMap::new(), handed: HashMap::new(), following: HashMap::new(), said: HashMap::new(), stopped_starting: false, environment: None });
            }
        }
        if let Some(window) = windows.get_mut(id) {
            window.environment = environment;
            window.default = default;
        }
        Ok(json!({"ok": true, "pid": std::process::id()}))
    }
}

impl ConversationHost for DaemonHost {
    fn open_sessions(&self) -> Vec<harness_daemon::OpenSession> {
        let windows = self.windows.lock().expect("windows");
        let data = self.data_dir.to_string_lossy();
        let mut out = Vec::new();
        for window in windows.values().filter(|w| !w.closed) {
            for (_, session) in window.sessions.open() {
                if !out.iter().any(|s: &harness_daemon::OpenSession| s.id == session) {
                    let nick = self.nicks.lock().expect("nicks").get(&session).cloned();
                    let title = nick.clone().unwrap_or_else(|| crate::title_of(&data, &window.root, &session));
                    out.push(harness_daemon::OpenSession { title, nick, id: session });
                }
            }
        }
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    fn deliver_to(&self, session: &str, label: &str, text: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.inbound(tab, label.to_string(), text.to_string());
                self.wake.notify_all();
                return true;
            }
        }
        false
    }

    fn notify(&self, session: &str, label: &str, text: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.notice(tab, label.to_string(), text.to_string());
                self.wake.notify_all();
                return true;
            }
        }
        false
    }

    fn session_of(&self, window: &str, tab: u64) -> Option<String> {
        self.windows.lock().expect("windows").get(window)?.sessions.session_of(tab).map(str::to_string)
    }

    fn rename(&self, session: &str, nick: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.nick(tab, nick.to_string());
                return true;
            }
        }
        false
    }

    fn close(&self, only_if_idle: bool) -> bool {
        let threads: Vec<_> = {
            // Under the lock every operation takes: no message starts a run between the look and the close.
            let mut windows = self.windows.lock().expect("windows");
            if only_if_idle && windows.values().any(|w| w.sessions.busy()) {
                return false;
            }
            windows.drain().flat_map(|(_, mut window)| window.sessions.close()).collect()
        };
        self.wake.notify_all();
        let end = Instant::now() + CLOSING;
        while Instant::now() < end && !threads.iter().all(|t| t.is_finished()) {
            std::thread::sleep(PUMP);
        }
        true
    }

    fn op(&self, id: &str, op: &Value) -> Result<Value, String> {
        let mut windows = self.windows.lock().expect("windows");
        let name = op["op"].as_str().ok_or("op is required")?;
        if name == "open" {
            return self.open(&mut windows, id, op);
        }
        if name == "busy" {
            return Ok(json!({"busy": windows.values().any(|w| w.sessions.busy())}));
        }
        // A debug call: the window the user is most likely looking at draws itself into a file.
        if name == "screenshot" {
            let path = op["text"].as_str().ok_or("text is required: the file to write")?;
            let window = windows.values_mut().filter(|w| !w.silent()).max_by_key(|w| w.polled).ok_or("no window is open")?;
            window.say(json!({"kind": "screenshot", "path": path}));
            self.wake.notify_all();
            return Ok(json!({"ok": true}));
        }
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        window.polled = Instant::now();
        let tab = op["tab"].as_u64().unwrap_or(0);
        // An agent asks for the harness to be rebuilt: the window does it, and tells that tab.
        if name == "reload" {
            window.say(json!({"kind": "reload", "tab": tab}));
            self.wake.notify_all();
            return Ok(json!({"ok": true}));
        }
        if name == "left" {
            let waiting = self.left.lock().expect("left").remove(op["text"].as_str().unwrap_or_default()).unwrap_or_default();
            return Ok(json!({"messages": waiting}));
        }
        if name == "choose" {
            let choice = crate::Choice { provider: op["provider"].as_str().ok_or("provider is required")?.to_string(), model: op["model"].as_str().ok_or("model is required")?.to_string() };
            return self.choose(window, tab, choice);
        }
        if name == "choice_of" {
            return Ok(json!({"choice": window.choices.lock().expect("choices").get(&tab)}));
        }
        if name == "resume" {
            let session = op["text"].as_str().unwrap_or_default();
            self.settle(window, tab, session);
            if let Some(reply) = Self::take_over(&mut windows, id, tab, session) {
                self.wake.notify_all();
                return Ok(reply);
            }
        } else if window.following.contains_key(&tab) {
            return Self::on_followed(&mut windows, id, tab, name, op);
        }
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        let reply = window.apply(name, op);
        // A first message made it a conversation.
        if let Some(session) = window.sessions.session_of(tab).map(str::to_string) {
            self.settle(window, tab, &session);
        }
        reply
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
    environment: std::collections::BTreeMap<String, String>,
    say: Sender<FromSession>,
    /// The tab whose agent asked for a reload.
    asked_by: Arc<std::sync::atomic::AtomicU64>,
    /// Where a debug call has asked this window to draw itself.
    shot: Arc<Mutex<Option<String>>>,
}

impl RemoteSessions {
    /// Opens this window in the daemon and starts listening. `reload` is raised when a session
    /// asks for a hot reload.
    pub fn open(client: Client, root: &str, settings: &Settings, reload: Arc<AtomicBool>) -> Result<RemoteSessions, String> {
        Self::open_in(client, root, settings, reload, crate::this_environment())
    }

    /// The same, for a window with this environment rather than this process's.
    pub fn open_in(client: Client, root: &str, settings: &Settings, reload: Arc<AtomicBool>, environment: std::collections::BTreeMap<String, String>) -> Result<RemoteSessions, String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let window = format!("conv-{}-{now}", std::process::id());
        client.conversation_op(&json!({"window": window, "op": "open", "root": root, "settings": settings, "env": environment}))?;
        let (sender, incoming) = channel();
        let (tabs, stopped): (Arc<Mutex<HashSet<u64>>>, Arc<AtomicBool>) = (Arc::default(), Arc::default());
        let asked_by: Arc<std::sync::atomic::AtomicU64> = Arc::default();
        let shot: Arc<Mutex<Option<String>>> = Arc::default();
        listen(client.clone(), window.clone(), sender.clone(), tabs.clone(), stopped.clone(), Asks { reload, asked_by: asked_by.clone(), shot: shot.clone() });
        Ok(RemoteSessions { client, window, incoming, tabs, stopped, environment, say: sender, asked_by, shot })
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
        self.client.conversation_op(&json!({"window": self.window, "op": "open", "root": root, "settings": settings, "env": self.environment})).map(|_| ())
    }
}

impl Drop for RemoteSessions {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _ = self.client.conversation_op(&json!({"window": self.window, "op": "close"}));
    }
}

/// What the daemon can ask of this window beyond its conversations.
struct Asks {
    reload: Arc<AtomicBool>,
    asked_by: Arc<std::sync::atomic::AtomicU64>,
    shot: Arc<Mutex<Option<String>>>,
}

fn listen(client: Client, window: String, sender: Sender<FromSession>, tabs: Arc<Mutex<HashSet<u64>>>, stopped: Arc<AtomicBool>, asks: Asks) {
    let Asks { reload, asked_by, shot } = asks;
    std::thread::spawn(move || {
        let (mut after, mut failures) = (0, 0);
        while !stopped.load(Ordering::SeqCst) {
            match client.conversation_events(&window, after) {
                Ok(events) => {
                    failures = 0;
                    for event in events {
                        after = event["seq"].as_u64().unwrap_or(after);
                        if event["event"]["kind"] == "screenshot" {
                            *shot.lock().expect("shot") = event["event"]["path"].as_str().map(str::to_string);
                        } else if event["event"]["kind"] == "reload" {
                            // Which tab asked, if the asking came from outside this process.
                            if let Some(tab) = event["event"]["tab"].as_u64() {
                                asked_by.store(tab, Ordering::SeqCst);
                            }
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

/// What opening a stored session came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The tab has it; its next message continues the log.
    Ready,
    /// Its run is on, and now speaks to this tab; what it says next comes as for any run.
    Attached,
    /// Another window has it open: said on the tab.
    Elsewhere,
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
                if s.busy() {
                    return Err("a run is on: the adapter can be changed when it ends".to_string());
                }
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

    pub fn resume(&mut self, tab: u64, session: &str) -> Opened {
        match self {
            SessionLink::Local(s) => {
                s.resume(tab, session);
                Opened::Ready
            }
            SessionLink::Remote(r) => {
                // The other window may have just gone: its silence shows within seconds.
                let deadline = Instant::now() + WAIT_FOR_OTHER;
                loop {
                    let Some(reply) = r.call("resume", tab, json!({"text": session})) else { return Opened::Ready };
                    if reply["attached"] == true {
                        return Opened::Attached;
                    }
                    if reply["elsewhere"] != true {
                        return Opened::Ready;
                    }
                    if Instant::now() >= deadline {
                        let _ = r.say.send(FromSession::Unavailable { tab, message: "This conversation is open in another window. Close it there to go on here: one writer to a log.".to_string() });
                        return Opened::Elsewhere;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    /// What was sent to this session from a window that went away before it could go in.
    pub fn left_waiting(&self, tab: u64, session: &str) -> Vec<String> {
        match self {
            SessionLink::Local(_) => Vec::new(),
            SessionLink::Remote(r) => r.call("left", tab, json!({"text": session})).map_or(Vec::new(), |reply| serde_json::from_value(reply["messages"].clone()).unwrap_or_default()),
        }
    }

    /// The tab whose agent asked for the reload in hand, if one did from outside this process.
    /// The file a debug call wants this window drawn into, once.
    pub fn screenshot_asked(&self) -> Option<String> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => r.shot.lock().expect("shot").take(),
        }
    }

    pub fn reload_asked_by(&self) -> Option<u64> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => Some(r.asked_by.swap(0, Ordering::SeqCst)).filter(|tab| *tab != 0),
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

    /// This tab's conversation goes on on this adapter, and no other moves. With no daemon there is
    /// no one to keep it: the tab takes the settings as they are now, like any tab started after.
    pub fn choose(&mut self, tab: u64, provider: &str, model: &str) -> Result<bool, String> {
        match self {
            SessionLink::Local(s) => Ok(s.rebind(tab)),
            SessionLink::Remote(r) => r.client.conversation_op(&json!({"window": r.window, "op": "choose", "tab": tab, "provider": provider, "model": model})).map(|reply| reply["ok"] == true),
        }
    }

    /// What this tab's conversation runs on, if the daemon has settled it.
    pub fn choice_of(&self, tab: u64) -> Option<crate::Choice> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => r.client.conversation_op(&json!({"window": r.window, "op": "choice_of", "tab": tab})).ok().and_then(|reply| serde_json::from_value(reply["choice"].clone()).ok()),
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
