//! A client's side of the daemon (CON-130, CON-131, ADR 0027): one connection to its local socket,
//! kept open and made again whenever it drops.
//!
//! A process has one connection per data directory: every [`Client`] found for it shares it. When the
//! connection drops, the client tries again every 2 s, forever, reading `address.json` afresh each
//! time so a replaced daemon is found; one that knows how to start the daemon starts it when none has
//! come within [`REFIND_WAIT`]. Requests made while it is down wait and go once it is back, in order;
//! a request already written when it dropped is answered with an error and not sent again, so nothing
//! is applied twice. [`Client::watch`] says each loss and each connection made, the first included.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::socket::{PROTOCOL, connect, socket_path};
use crate::{Address, Numbered, address_path};

/// How long a client that can start the daemon waits for one to appear before it starts one.
pub const REFIND_WAIT: Duration = Duration::from_secs(5);

/// The pause between two tries to reach the daemon (CON-131).
pub const RETRY: Duration = Duration::from_secs(2);

/// What a window's call says when there is no connection (CON-131, CON-132).
pub const NOT_CONNECTED: &str = "no connection to the daemon";

/// A registered window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registered {
    pub window: String,
    pub url: String,
}

/// What a client's connection does, for a window to show (CON-131).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    /// Connected to the daemon of this pid; `again` after a loss.
    Connected { pid: u64, again: bool },
    /// The connection dropped; the client is trying again every 2 s.
    Lost,
    /// A try that did not connect: the how-manyth since the connection was last made, and why
    /// when it is known (CON-131).
    Trying { attempt: u64, reason: String },
}

/// Run on every connection made after the first, before anything waiting is sent: a window opens
/// itself again in a daemon that is new. `new_daemon` says whether it is another daemon.
pub type OnConnect = Arc<dyn Fn(&Client, bool) + Send + Sync>;

/// Starts the daemon detached: it outlives whoever started it, and is nobody's child to wait for.
#[cfg(unix)]
fn start_daemon(program: &Path, data_dir: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    // A daemon run by a user service is started by it, never beside it (journal#1181).
    if let Some(kind) = crate::service::installed_for(data_dir) {
        return crate::service::start(kind).map_err(std::io::Error::other);
    }
    // Through a shell that ends at once and is waited for here, so the daemon is never left
    // defunct under a window that goes on for days.
    // In a session of its own (journal#956): no controlling terminal, so nothing it or its programs
    // write to /dev/tty lands in the window that started it, and closing that window's terminal
    // does not hang it up.
    let mut shell = std::process::Command::new("sh");
    shell.args(["-c", "\"$0\" daemon </dev/null >/dev/null 2>&1 &"]).arg(program);
    // SAFETY: setsid is async-signal-safe, and the shell is not a process group leader yet.
    unsafe {
        shell.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) });
    }
    shell.status().map(|_| ())
}

#[cfg(windows)]
fn start_daemon(program: &Path, _data_dir: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let mut command = std::process::Command::new(program);
    command.arg("daemon").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    command.creation_flags(DETACHED_PROCESS);
    command.spawn().map(|_| ())
}

/// The connection as it stands.
#[derive(Default)]
struct Link {
    writer: Option<socket2::Socket>,
    /// Open to everyone; before it, only the on-connect hooks may send.
    ready: bool,
    pid: u64,
    port: u16,
    build: String,
    /// Which daemon: a new one, even in the same process, has another.
    instance: String,
    /// Connections made so far.
    made: u64,
    down_since: Option<Instant>,
}

/// Where the reply to a request goes: its status and body, or why none will come.
type Answer = Sender<Result<(u16, Value), String>>;

struct Sub {
    kind: String,
    window: String,
    after: u64,
    to: Sender<Vec<Value>>,
}

struct Inner {
    dir: Option<PathBuf>,
    /// A socket given by an address rather than found through a data directory.
    fixed: Option<PathBuf>,
    role: String,
    program: Mutex<Option<PathBuf>>,
    link: Mutex<Link>,
    changed: Condvar,
    next: AtomicU64,
    pending: Mutex<HashMap<u64, Answer>>,
    subs: Mutex<HashMap<u64, Sub>>,
    watchers: Mutex<Vec<Sender<Connection>>>,
    hooks: Mutex<Vec<OnConnect>>,
    /// Tries since the connection was last made.
    attempts: AtomicU64,
    /// Raised by someone who looks for the daemon now: the next try does not wait out its 2 s.
    kick: Mutex<bool>,
    kicked: Condvar,
}

impl Inner {
    /// The pause between tries: 2 s, unless someone looks for the daemon meanwhile.
    fn pause(&self) {
        let mut kick = self.kick.lock().expect("kick");
        if !*kick {
            kick = self.kicked.wait_timeout(kick, RETRY).expect("kick").0;
        }
        *kick = false;
    }

    fn try_now(&self) {
        *self.kick.lock().expect("kick") = true;
        self.kicked.notify_all();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let Ok(link) = self.link.get_mut()
            && let Some(writer) = &link.writer
        {
            let _ = writer.shutdown(std::net::Shutdown::Both);
        }
    }
}

/// Set in a window's process (CON-132): its calls never wait for a connection, and wait at most
/// [`REPLY_WAIT`] for a reply; other processes (session servers, the command line) wait (CON-131).
static WINDOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long a window waits for a reply before it counts the daemon as lost (CON-132).
pub const REPLY_WAIT: Duration = Duration::from_secs(2);

/// This process is a window: it must never hang on the daemon (CON-132).
pub fn this_is_a_window() {
    WINDOW.store(true, Ordering::SeqCst);
}

thread_local! {
    /// Set while an on-connect hook runs: its requests go before the link is open to everyone.
    static IN_HOOK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The one connection of this process to each data directory's daemon.
static SHARED: Mutex<Option<HashMap<PathBuf, Weak<Inner>>>> = Mutex::new(None);

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    fn spawn(dir: Option<PathBuf>, fixed: Option<PathBuf>, program: Option<PathBuf>) -> Client {
        let role = std::env::var("MULTITOOL_CLIENT_ROLE").unwrap_or_else(|_| "window".to_string());
        let inner = Arc::new(Inner {
            dir,
            fixed,
            role,
            program: Mutex::new(program),
            link: Mutex::new(Link::default()),
            changed: Condvar::new(),
            next: AtomicU64::new(1),
            pending: Mutex::new(HashMap::new()),
            subs: Mutex::new(HashMap::new()),
            watchers: Mutex::new(Vec::new()),
            hooks: Mutex::new(Vec::new()),
            attempts: AtomicU64::new(0),
            kick: Mutex::new(false),
            kicked: Condvar::new(),
        });
        let weak = Arc::downgrade(&inner);
        std::thread::spawn(move || keep_connected(weak));
        Client { inner }
    }

    /// A client of the daemon at this address's socket, on its own connection (tests).
    pub fn new(address: &Address) -> Client {
        Client::spawn(None, Some(address.socket.clone().unwrap_or_default()), None)
    }

    /// The daemon of this data directory, if one answers now; its connection is the process's one.
    pub fn find(data_dir: &Path) -> Option<Client> {
        // Nothing listens: said at once, with no connection kept trying for it.
        let known = SHARED.lock().expect("clients").as_ref().and_then(|m| m.get(data_dir).and_then(Weak::upgrade)).is_some();
        if !known && !socket_answers(data_dir) {
            return None;
        }
        let client = Client::shared(data_dir, None);
        if !client.current() {
            client.inner.try_now();
        }
        client.wait_ready(Duration::from_secs(2)).then_some(client)
    }

    fn shared(data_dir: &Path, program: Option<&Path>) -> Client {
        let key = data_dir.to_path_buf();
        let mut shared = SHARED.lock().expect("clients");
        let map = shared.get_or_insert_with(HashMap::new);
        let client = match map.get(&key).and_then(Weak::upgrade) {
            Some(inner) => Client { inner },
            None => {
                let client = Client::spawn(Some(key.clone()), None, None);
                map.insert(key, Arc::downgrade(&client.inner));
                client
            }
        };
        if let Some(program) = program {
            *client.inner.program.lock().expect("program") = Some(program.to_path_buf());
        }
        client
    }

    /// Finds the daemon, or starts `program daemon` detached from this terminal and waits for it.
    /// From then on this process's connection starts it again whenever it is gone (CON-131).
    pub fn find_or_start(data_dir: &Path, program: &Path) -> Result<Client, String> {
        let client = Client::shared(data_dir, Some(program));
        if !client.current() {
            client.inner.try_now();
        }
        if client.wait_ready(Duration::from_millis(300)) {
            return Ok(client);
        }
        // Never a second daemon beside one of another protocol: that one is waited for (CON-130).
        if other_protocol(data_dir).is_some() {
            return Err("a daemon of another protocol holds this data directory".to_string());
        }
        if !socket_answers(data_dir) {
            start_daemon(program, data_dir).map_err(|e| format!("starting the daemon: {e}"))?;
        }
        // A new daemon may wait up to 10 s for its last port to be let go (journal#1054).
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            client.inner.try_now();
            if client.wait_ready(Duration::from_millis(100)) {
                return Ok(client);
            }
        }
        Err("the daemon did not start".to_string())
    }

    /// This process's connection to the daemon of `data_dir`, connected or not yet: it keeps trying,
    /// and starts `program`'s daemon when none answers (CON-130, CON-131).
    pub fn for_window(data_dir: &Path, program: &Path) -> Client {
        let client = Client::shared(data_dir, Some(program));
        client.inner.try_now();
        client
    }

    fn wait_ready(&self, within: Duration) -> bool {
        let end = Instant::now() + within;
        let mut link = self.inner.link.lock().expect("link");
        while !link.ready {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            link = self.inner.changed.wait_timeout(link, left).expect("link").0;
        }
        true
    }

    /// Whether it is connected now.
    pub fn current(&self) -> bool {
        self.inner.link.lock().is_ok_and(|l| l.ready)
    }

    /// The daemon now answering: this same client, which reconnects by itself, once it is back.
    pub fn refind(&self, _program: Option<&Path>) -> Option<Client> {
        self.wait_ready(REFIND_WAIT + RETRY).then(|| self.clone())
    }

    /// Hears each loss and each connection made; the state now comes first.
    pub fn watch(&self) -> Receiver<Connection> {
        let (to, from) = channel();
        let link = self.inner.link.lock().expect("link");
        if link.ready {
            let _ = to.send(Connection::Connected { pid: link.pid, again: false });
        }
        self.inner.watchers.lock().expect("watchers").push(to);
        from
    }

    /// Runs `hook` on every connection made from now on, before what waits is sent.
    pub fn on_connect(&self, hook: OnConnect) {
        self.inner.hooks.lock().expect("hooks").push(hook);
    }

    /// The daemon's pid, while connected.
    pub fn pid(&self) -> Option<u64> {
        self.inner.link.lock().ok().filter(|l| l.ready).map(|l| l.pid)
    }

    /// One request. In a window it fails at once when there is no connection and waits at most
    /// [`REPLY_WAIT`] for its reply, after which the daemon counts as lost (CON-132); elsewhere it
    /// waits while the connection is down (CON-131).
    pub fn request(&self, method: &str, path: &str, query: &str, body: &str) -> Result<(u16, Value), String> {
        let id = self.inner.next.fetch_add(1, Ordering::SeqCst);
        let (to, from) = channel();
        let in_hook = IN_HOOK.with(|h| h.get());
        let window = WINDOW.load(Ordering::SeqCst) && !in_hook;
        {
            let mut link = self.inner.link.lock().expect("link");
            while !(link.ready || in_hook && link.writer.is_some()) {
                if window {
                    return Err(NOT_CONNECTED.to_string());
                }
                link = self.inner.changed.wait(link).expect("link");
            }
            self.inner.pending.lock().expect("pending").insert(id, to);
            let line = json!({"id": id, "method": method, "path": path, "query": query, "body": body}).to_string() + "\n";
            let written = link.writer.as_ref().map(|mut w| w.write_all(line.as_bytes()));
            if !matches!(written, Some(Ok(()))) {
                self.inner.pending.lock().expect("pending").remove(&id);
                return Err("the connection to the daemon dropped".to_string());
            }
        }
        // A stop waits for the daemon's runs to close: it may take longer, and it ends the daemon anyway.
        if !window || path == "/shutdown" {
            return from.recv().unwrap_or_else(|_| Err("the connection to the daemon dropped before it answered".to_string()));
        }
        match from.recv_timeout(REPLY_WAIT) {
            Ok(answer) => answer,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err("the connection to the daemon dropped before it answered".to_string()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // A daemon that does not answer is as good as gone: the connection is closed, so the
                // reconnect loop and its toasts start and the next calls fail at once (CON-132).
                self.inner.pending.lock().expect("pending").remove(&id);
                if let Some(writer) = self.inner.link.lock().expect("link").writer.as_ref() {
                    let _ = writer.shutdown(std::net::Shutdown::Both);
                }
                Err(format!("the daemon did not answer in {} s", REPLY_WAIT.as_secs()))
            }
        }
    }

    /// The reply of a request that has no other success than 200, as JSON.
    fn ok(&self, method: &str, path: &str, body: &str) -> Result<Value, String> {
        let (status, reply) = self.request(method, path, "", body)?;
        if status != 200 {
            return Err(reply["error"].as_str().map_or_else(|| format!("the daemon answered {status}"), str::to_string));
        }
        Ok(reply)
    }

    /// A window's events from now on, pushed by the daemon, each batch as it comes: `kind` is
    /// `events` (callbacks and the like) or `conversations` (a window's sessions). The receiver
    /// ends when the window is gone.
    pub fn subscribe(&self, kind: &str, window: &str, after: u64) -> Receiver<Vec<Value>> {
        let id = self.inner.next.fetch_add(1, Ordering::SeqCst);
        let (to, from) = channel();
        self.inner.subs.lock().expect("subscriptions").insert(id, Sub { kind: kind.to_string(), window: window.to_string(), after, to });
        let link = self.inner.link.lock().expect("link");
        if let Some(mut writer) = link.writer.as_ref() {
            let _ = writer.write_all((json!({"id": id, "subscribe": kind, "window": window, "after": after}).to_string() + "\n").as_bytes());
        }
        from
    }

    /// One operation on this window's conversations.
    pub fn conversation_op(&self, op: &Value) -> Result<Value, String> {
        self.ok("POST", "/conversations", &op.to_string())
    }

    /// The build of the daemon that answers.
    pub fn build(&self) -> Option<String> {
        self.wait_ready(Duration::from_secs(2)).then(|| self.inner.link.lock().expect("link").build.clone())
    }

    pub fn register(&self, root: &str) -> Result<Registered, String> {
        let body = self.ok("POST", "/register", &json!({"root": root}).to_string()).map_err(|e| format!("registering with the daemon: {e}"))?;
        match (body["window"].as_str(), body["url"].as_str()) {
            (Some(window), Some(url)) => Ok(Registered { window: window.to_string(), url: url.to_string() }),
            _ => Err(format!("the daemon refused: {body}")),
        }
    }

    /// The daemon's HTTP port, where callbacks come from outside.
    pub fn port(&self) -> String {
        let link = self.inner.link.lock().expect("link");
        link.port.to_string()
    }

    /// Asks the daemon to stop, and waits until that connection is gone.
    pub fn shutdown(&self) -> Result<(), String> {
        let before = self.inner.link.lock().expect("link").made;
        self.ok("POST", "/shutdown", "{}").map_err(|e| format!("reaching the daemon: {e}"))?;
        self.wait_gone(before)
    }

    fn wait_gone(&self, made: u64) -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs(5);
        let mut link = self.inner.link.lock().expect("link");
        while link.ready && link.made == made {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the daemon did not stop".to_string());
            }
            link = self.inner.changed.wait_timeout(link, left).expect("link").0;
        }
        Ok(())
    }

    /// Stops the daemon unless a run is on in it, which answers false: the daemon looks and closes
    /// in one step, so nothing starts between the two.
    pub fn shutdown_if_idle(&self) -> Result<bool, String> {
        let before = self.inner.link.lock().expect("link").made;
        let (status, _) = self.request("POST", "/shutdown", "", r#"{"if_idle":true}"#).map_err(|e| format!("reaching the daemon: {e}"))?;
        if status == 409 {
            return Ok(false);
        }
        self.wait_gone(before).map(|_| true)
    }

    /// The custom handlers the daemon's build has: (name, types).
    pub fn handlers(&self) -> Result<Vec<(String, Vec<String>)>, String> {
        let body = self.ok("GET", "/handlers", "")?;
        Ok(body["handlers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|h| (h["name"].as_str().unwrap_or_default().to_string(), h["types"].as_array().into_iter().flatten().filter_map(|t| t.as_str().map(str::to_string)).collect()))
            .collect())
    }

    /// The daemon reads the projects' callback files again.
    pub fn reload(&self) -> Result<Value, String> {
        self.request("POST", "/reload", "", "{}").map(|(_, reply)| reply)
    }

    /// Sends to the operator; `from`, a window and its tab, answers where that session is bound.
    /// A plan for the user to approve on Telegram with buttons (CON-161).
    pub fn telegram_plan(&self, session: &str, title: &str, plan: &str) -> Result<(), String> {
        self.request("POST", "/telegram/plan", "", &json!({"session": session, "title": title, "plan": plan}).to_string()).map(|_| ())
    }

    /// A plan answered in the window: its Telegram message follows (CON-163).
    pub fn telegram_plan_done(&self, session: &str, approved: bool) -> Result<(), String> {
        self.request("POST", "/telegram/plan_done", "", &json!({"session": session, "approved": approved}).to_string()).map(|_| ())
    }

    pub fn telegram_send(&self, text: &str, files: &[std::path::PathBuf], from: Option<(&str, u64)>) -> Result<u64, String> {
        let (_, body) = self.request("POST", "/telegram/send", "", &json!({"text": text, "files": files, "window": from.map(|f| f.0), "tab": from.map(|f| f.1)}).to_string())?;
        match body["error"].as_str() {
            Some(error) => Err(error.to_string()),
            None => Ok(body["parts"].as_u64().unwrap_or(0)),
        }
    }

    /// The Telegram channel takes up its settings as they now stand.
    pub fn telegram_reload(&self) -> Result<Value, String> {
        self.request("POST", "/telegram/reload", "", "{}").map(|(_, reply)| reply)
    }

    /// The window's session entered or left step-debug mode.
    pub fn set_step_debug(&self, window: &str, on: bool) -> Result<(), String> {
        let (status, _) = self.request("POST", "/window/step-debug", "", &json!({"window": window, "on": on}).to_string())?;
        if status == 200 { Ok(()) } else { Err(format!("the daemon answered {status}")) }
    }

    /// What the daemon counts, for a look from outside.
    pub fn stats(&self) -> Result<Value, String> {
        self.request("GET", "/stats", "", "").map(|(_, reply)| reply)
    }

    /// One Memory call. It waits while the daemon is away (CON-131); `Failed` is the daemon's answer,
    /// or a connection that dropped after the call went out, which is not sent again.
    pub fn memory(&self, request: &crate::memory::Request) -> Result<Value, crate::memory::CallError> {
        use crate::memory::CallError;
        let body = serde_json::to_string(request).map_err(|e| CallError::Failed(e.to_string()))?;
        let (_, mut reply) = self.request("POST", "/memory", "", &body).map_err(CallError::Failed)?;
        match reply["error"].as_str() {
            Some(error) => Err(CallError::Failed(error.to_string())),
            None => Ok(reply["result"].take()),
        }
    }

    /// Events after `after`, waiting a while for some: one held request (tests and one-off readers;
    /// a window subscribes instead).
    pub fn poll(&self, window: &str, after: u64) -> Result<Vec<Numbered>, String> {
        let (status, body) = self.request("GET", "/events", &format!("window={window}&after={after}"), "")?;
        if status != 200 {
            return Err(body["error"].as_str().unwrap_or("polling the daemon failed").to_string());
        }
        serde_json::from_value(body["events"].clone()).map_err(|e| format!("the daemon's events: {e}"))
    }

    /// A window's conversation events after `after`, one held request (tests).
    pub fn conversation_events(&self, window: &str, after: u64) -> Result<Vec<Value>, String> {
        let (status, body) = self.request("GET", "/conversations/events", &format!("window={window}&after={after}"), "")?;
        if status != 200 {
            return Err(body["error"].as_str().unwrap_or("a bad answer from the daemon").to_string());
        }
        body["events"].as_array().cloned().ok_or_else(|| "a bad answer from the daemon".to_string())
    }
}

/// Whether the daemon of this address still answers, whatever it speaks.
pub fn other_answers(address: &Address) -> bool {
    if let Some(path) = &address.socket
        && connect(path).is_ok()
    {
        return true;
    }
    let quick: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(2))).build().into();
    address.port != 0 && quick.get(format!("http://127.0.0.1:{}/health", address.port)).call().is_ok_and(|r| r.status() == 200)
}

/// Asks a daemon of another protocol to stop if nothing runs in it; true when it agreed. One from
/// before the socket is asked over HTTP with its token; one with a socket, by the stop every
/// protocol understands.
pub fn stop_if_idle(address: &Address) -> bool {
    if let Some(path) = &address.socket
        && let Ok(socket) = connect(path)
    {
        return crate::socket::ask_stop_if_idle(socket).unwrap_or(false);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(10))).build().into();
    agent
        .post(format!("http://127.0.0.1:{}/shutdown", address.port))
        .header("authorization", &format!("Bearer {}", address.admin))
        .send(r#"{"if_idle":true}"#)
        .is_ok_and(|r| r.status() == 200)
}

/// The address of a daemon of another protocol that still answers for `data_dir`, if there is one.
pub fn other_protocol(data_dir: &Path) -> Option<Address> {
    std::fs::read_to_string(address_path(data_dir)).ok().and_then(|t| serde_json::from_str::<Address>(&t).ok()).filter(|a| a.protocol != PROTOCOL && other_answers(a))
}

/// Whether something answers on the socket of this data directory, without the protocol.
fn socket_answers(data_dir: &Path) -> bool {
    socket_of(data_dir).is_some_and(|path| connect(&path).is_ok())
}

/// Where the daemon of `data_dir` listens: what `address.json` names, else the usual place.
fn socket_of(data_dir: &Path) -> Option<PathBuf> {
    let named = std::fs::read_to_string(address_path(data_dir)).ok().and_then(|text| serde_json::from_str::<Address>(&text).ok()).and_then(|a| a.socket);
    Some(named.unwrap_or_else(|| socket_path(data_dir)))
}

type Lines = std::io::Lines<BufReader<socket2::Socket>>;

/// One try: connect, hello, welcome. The lines go on from there.
fn dial(inner: &Inner) -> Result<(socket2::Socket, Lines, Value), String> {
    let path = match (&inner.fixed, &inner.dir) {
        (Some(path), _) => path.clone(),
        (None, Some(dir)) => socket_of(dir).ok_or("no data directory")?,
        (None, None) => return Err("nowhere to connect".to_string()),
    };
    let socket = connect(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    // A daemon that took the connection and never welcomes it is given up on, and tried again.
    socket.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
    let mut writer = socket.try_clone().map_err(|e| e.to_string())?;
    writer
        .write_all((json!({"hello": {"protocol": PROTOCOL, "build": crate::build_id(), "role": inner.role}}).to_string() + "\n").as_bytes())
        .map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(socket).lines();
    let first: Value = serde_json::from_str(&lines.next().ok_or("closed at once")?.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    if first.get("welcome").is_none() {
        return Err(first["refused"]["error"].as_str().unwrap_or("refused").to_string());
    }
    // The same socket under both handles: the reader waits as long as it must from now on.
    writer.set_read_timeout(None).map_err(|e| e.to_string())?;
    Ok((writer, lines, first["welcome"].clone()))
}

/// The connection's own thread: makes it, hands its lines to a reader, and makes it again when it
/// drops, every 2 s, for as long as anyone holds the client.
fn keep_connected(weak: Weak<Inner>) {
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let attempt = dial(&inner);
        let Ok((writer, lines, welcome)) = attempt else {
            let program = inner.program.lock().expect("program").clone();
            let mut reason = "no daemon answers".to_string();
            // A daemon of another protocol holds the directory: asked to stop if idle, every try;
            // while it runs other windows' conversations this window waits for it (CON-130).
            let other = inner.dir.as_deref().and_then(other_protocol);
            let mut freed = false;
            if let Some(old) = other {
                if program.is_some() && stop_if_idle(&old) {
                    let end = Instant::now() + Duration::from_secs(5);
                    while other_answers(&old) && Instant::now() < end {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    freed = true;
                    reason = format!("a daemon of protocol {} was idle and is being replaced", old.protocol);
                } else {
                    reason = format!("a daemon of protocol {} (another build) runs other windows' conversations: waiting for it to be idle", old.protocol);
                }
            }
            // Nobody has come back: one who can start the daemon does, once it has waited, or at
            // once in place of one it just stopped.
            let since = inner.link.lock().expect("link").down_since;
            if let (Some(program), Some(dir)) = (program, &inner.dir)
                && (freed || since.is_some_and(|s| s.elapsed() >= REFIND_WAIT))
                && !socket_answers(dir)
                && (freed || other_protocol(dir).is_none())
            {
                let _ = start_daemon(&program, dir);
                inner.link.lock().expect("link").down_since = Some(Instant::now());
                // The one just started is connected to as soon as it listens, not a try later.
                let end = Instant::now() + Duration::from_secs(3);
                while !socket_answers(dir) && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(50));
                }
                inner.try_now();
            }
            if inner.link.lock().expect("link").down_since.is_none() {
                inner.link.lock().expect("link").down_since = Some(Instant::now());
            }
            let attempt = inner.attempts.fetch_add(1, Ordering::SeqCst) + 1;
            tell(&inner, Connection::Trying { attempt, reason });
            if freed {
                inner.try_now();
            }
            inner.pause();
            drop(inner);
            continue;
        };
        inner.attempts.store(0, Ordering::SeqCst);
        let pid = welcome["pid"].as_u64().unwrap_or(0);
        let instance = welcome["instance"].as_str().unwrap_or_default().to_string();
        debug(&format!("welcome {instance}"));
        let (again, new_daemon) = {
            let mut link = inner.link.lock().expect("link");
            let again = link.made > 0;
            let new_daemon = link.instance != instance;
            link.instance = instance;
            link.writer = Some(writer);
            link.pid = pid;
            link.port = welcome["port"].as_u64().unwrap_or(0) as u16;
            link.build = welcome["build"].as_str().unwrap_or_default().to_string();
            link.made += 1;
            link.down_since = None;
            (again, new_daemon)
        };
        // Replies and pushes come in on their own thread from now on.
        let reader = {
            let weak = weak.clone();
            std::thread::spawn(move || read(weak, lines))
        };
        // A window opens itself in a daemon it reaches for the first time, before it hears or sends
        // anything there; the hooks say whether it is another daemon.
        {
            let hooks = inner.hooks.lock().expect("hooks").clone();
            let client = Client { inner: inner.clone() };
            IN_HOOK.with(|h| h.set(true));
            for hook in hooks {
                hook(&client, new_daemon);
            }
            IN_HOOK.with(|h| h.set(false));
        }
        // Every subscription again; a new daemon numbers a window's conversation events afresh.
        {
            let mut subs = inner.subs.lock().expect("subscriptions");
            let link = inner.link.lock().expect("link");
            for (id, sub) in subs.iter_mut() {
                if new_daemon && again && sub.kind == "conversations" {
                    sub.after = 0;
                }
                if let Some(mut writer) = link.writer.as_ref() {
                    let _ = writer.write_all((json!({"id": id, "subscribe": sub.kind, "window": sub.window, "after": sub.after}).to_string() + "\n").as_bytes());
                }
            }
        }
        inner.link.lock().expect("link").ready = true;
        inner.changed.notify_all();
        tell(&inner, Connection::Connected { pid, again });
        drop(inner);
        let _ = reader.join();
        let Some(inner) = weak.upgrade() else { return };
        {
            let mut link = inner.link.lock().expect("link");
            link.writer = None;
            link.ready = false;
            link.down_since = Some(Instant::now());
        }
        // What was written and not answered will never be: said so, never sent again.
        for (_, waiting) in inner.pending.lock().expect("pending").drain() {
            let _ = waiting.send(Err("the connection to the daemon dropped before it answered".to_string()));
        }
        inner.changed.notify_all();
        tell(&inner, Connection::Lost);
        inner.pause();
        drop(inner);
    }
}

fn debug(what: &str) {
    if std::env::var_os("MULTITOOL_CLIENT_DEBUG").is_some() {
        eprintln!("[client {:?}] {what}", std::time::SystemTime::now());
    }
}

fn tell(inner: &Inner, what: Connection) {
    debug(&format!("{what:?}"));
    inner.watchers.lock().expect("watchers").retain(|w| w.send(what.clone()).is_ok());
}

/// Hands each line to whoever waits for it: a reply to its request, a push to its subscription.
fn read(weak: Weak<Inner>, lines: Lines) {
    for line in lines {
        let Ok(line) = line else { break };
        let Some(inner) = weak.upgrade() else { return };
        let Ok(mut message) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = message["id"].as_u64() else { continue };
        if let Some(events) = message.get_mut("events").map(Value::take) {
            let mut subs = inner.subs.lock().expect("subscriptions");
            let gone = match subs.get_mut(&id) {
                Some(sub) => {
                    let events: Vec<Value> = events.as_array().cloned().unwrap_or_default();
                    sub.after = events.iter().filter_map(|e| e["seq"].as_u64()).max().unwrap_or(sub.after).max(sub.after);
                    sub.to.send(events).is_err()
                }
                None => false,
            };
            if gone {
                subs.remove(&id);
                if let Some(mut writer) = inner.link.lock().expect("link").writer.as_ref() {
                    let unsubscribe = inner.next.fetch_add(1, Ordering::SeqCst);
                    let _ = writer.write_all((json!({"id": unsubscribe, "unsubscribe": id}).to_string() + "\n").as_bytes());
                }
            }
            continue;
        }
        let status = message["status"].as_u64().unwrap_or(0) as u16;
        if let Some(waiting) = inner.pending.lock().expect("pending").remove(&id) {
            let _ = waiting.send(Ok((status, message["reply"].take())));
        } else if status == 404 {
            // A subscription the daemon ended because its window is gone: its receiver ends with it.
            // Anything else, a daemon stopping included, keeps it for the next connection.
            inner.subs.lock().expect("subscriptions").remove(&id);
        }
    }
}
