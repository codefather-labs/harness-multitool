//! The daemon's local socket (CON-130, ADR 0027): how windows, session servers and the command line
//! talk to it. One AF_UNIX socket on every OS, beside `address.json`; only its user can open it,
//! so what comes in is trusted as the admin token is over HTTP. Newline-delimited JSON:
//!
//! - the client opens with `{"hello": {"protocol": 1, "build": "...", "role": "window"}}` and is
//!   answered `{"welcome": {"protocol": 1, "pid": N, "build": "..."}}`, or `{"refused": {...}}` and
//!   the connection closes when the protocols differ;
//! - a request `{"id": 7, "method": "POST", "path": "/conversations", "query": "", "body": "..."}`
//!   is answered `{"id": 7, "status": 200, "reply": {...}}`;
//! - `{"id": 8, "subscribe": "events" | "conversations", "window": "W", "after": 0}` makes the daemon
//!   push `{"id": 8, "events": [...]}` whenever that window has some, until `{"id": 9, "unsubscribe":
//!   8}`, the connection closes, or a push `{"id": 8, "status": 404, "reply": {...}}` ends it.
//!
//! Every connection has its own reader, writer and queue; every request runs on a thread of its own,
//! so a slow or stuck client holds up no other. A client whose queue fills is cut off.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use socket2::{Domain, SockAddr, Socket, Type};

use crate::Daemon;

/// The protocol a window and a daemon must share to work together, whatever their builds.
pub const PROTOCOL: u32 = 1;

/// How many messages wait for a client before it counts as stuck and is cut off.
const QUEUE: usize = 4096;

/// Where the daemon of a data directory listens (CON-130).
pub fn socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("daemon.sock")
}

fn new_socket() -> std::io::Result<Socket> {
    Socket::new(Domain::UNIX, Type::STREAM, None)
}

/// A connection to the socket at `path`, if something listens there.
pub fn connect(path: &Path) -> std::io::Result<Socket> {
    let socket = new_socket()?;
    socket.connect(&SockAddr::unix(path)?)?;
    Ok(socket)
}

/// Asks the daemon on `socket` to stop if nothing runs in it, whatever protocol it speaks; true
/// when it agreed.
pub fn ask_stop_if_idle(socket: Socket) -> std::io::Result<bool> {
    let mut writer = socket.try_clone()?;
    writer.write_all(b"{\"stop_if_idle\": true}\n")?;
    let mut line = String::new();
    BufReader::new(socket).read_line(&mut line)?;
    Ok(serde_json::from_str::<Value>(&line).is_ok_and(|v| v["stopped"] == true))
}

/// The listening side, held by the daemon.
pub struct Server {
    path: PathBuf,
    stopped: AtomicBool,
    next: AtomicU64,
    /// Every open connection, to close them all when the daemon stops.
    open: Mutex<HashMap<u64, Socket>>,
}

impl Server {
    /// Binds the socket of `data_dir`, refusing if a daemon already answers on it.
    pub fn bind(data_dir: &Path) -> Result<(Arc<Server>, Socket), String> {
        let path = socket_path(data_dir);
        if connect(&path).is_ok() {
            return Err(format!("a daemon already listens on {}", path.display()));
        }
        // A file left by a daemon that is gone.
        let _ = std::fs::remove_file(&path);
        let address = SockAddr::unix(&path).map_err(|e| format!("the socket's path {} cannot be a socket address ({e}): use a shorter data directory", path.display()))?;
        let listener = new_socket().map_err(|e| format!("making the socket: {e}"))?;
        listener.bind(&address).map_err(|e| format!("binding {}: {e}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("making {} private: {e}", path.display()))?;
        }
        listener.listen(128).map_err(|e| format!("listening on {}: {e}", path.display()))?;
        Ok((Arc::new(Server { path, stopped: AtomicBool::new(false), next: AtomicU64::new(1), open: Mutex::new(HashMap::new()) }), listener))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes no more connections, closes every open one, so its client sees the loss at once, and
    /// removes the socket file.
    pub fn stop(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        // Wakes the accepting thread, which then sees it is stopped.
        let _ = connect(&self.path);
        for (_, socket) in self.open.lock().expect("connections").drain() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        let _ = std::fs::remove_file(&self.path);
    }

    pub fn connections(&self) -> usize {
        self.open.lock().map_or(0, |o| o.len())
    }
}

/// Accepts connections until the server stops, each served on threads of its own.
pub fn serve(daemon: Arc<Daemon>, server: Arc<Server>, listener: Socket) {
    std::thread::spawn(move || {
        while let Ok((socket, _)) = listener.accept() {
            if server.stopped.load(Ordering::SeqCst) {
                break;
            }
            let id = server.next.fetch_add(1, Ordering::SeqCst);
            if let Ok(copy) = socket.try_clone() {
                server.open.lock().expect("connections").insert(id, copy);
            }
            let (daemon, server) = (daemon.clone(), server.clone());
            std::thread::spawn(move || {
                connection(&daemon, socket);
                server.open.lock().expect("connections").remove(&id);
            });
        }
    });
}

/// What a connection's threads share.
struct Conn {
    out: SyncSender<String>,
    /// Messages queued and written, so a reply can be known to be out before the socket closes.
    queued: AtomicU64,
    written: AtomicU64,
    alive: AtomicBool,
    socket: Socket,
    /// Subscriptions by their id: false once unsubscribed.
    subs: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

impl Conn {
    /// Queues one message; a client that does not read is cut off rather than let it grow.
    fn send(&self, message: Value) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match self.out.try_send(message.to_string()) {
            Ok(()) => {
                self.queued.fetch_add(1, Ordering::SeqCst);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.close();
                false
            }
        }
    }

    /// Waits, a little, until everything queued so far is written.
    fn flush(&self) {
        let target = self.queued.load(Ordering::SeqCst);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while self.written.load(Ordering::SeqCst) < target && self.alive.load(Ordering::SeqCst) && std::time::Instant::now() < end {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn close(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

fn writer(mut socket: Socket, queue: Receiver<String>, conn: Arc<Conn>) {
    for line in queue {
        if socket.write_all(line.as_bytes()).and_then(|_| socket.write_all(b"\n")).is_err() {
            break;
        }
        conn.written.fetch_add(1, Ordering::SeqCst);
    }
    conn.close();
}

fn connection(daemon: &Arc<Daemon>, socket: Socket) {
    let (Ok(read_side), Ok(write_side), Ok(control)) = (socket.try_clone(), socket.try_clone(), socket.try_clone()) else { return };
    let (out, queue) = sync_channel(QUEUE);
    let conn = Arc::new(Conn { out, queued: AtomicU64::new(0), written: AtomicU64::new(0), alive: AtomicBool::new(true), socket: control, subs: Mutex::new(HashMap::new()) });
    {
        let conn = conn.clone();
        std::thread::spawn(move || writer(write_side, queue, conn));
    }
    let mut lines = BufReader::new(read_side).lines();
    // The hello first: a client of another protocol is told which this one speaks, and let go.
    let Some(Ok(first)) = lines.next() else { return conn.close() };
    let hello: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
    // The one thing every protocol understands: a window of another stops this daemon if it is idle,
    // to put its own in its place (CON-130).
    if hello.get("stop_if_idle").is_some() {
        let (status, _) = daemon.route("POST", "/shutdown", "", r#"{"if_idle":true}"#, true);
        conn.send(json!({"stopped": status == 200}));
        conn.flush();
        if daemon.stopping() {
            daemon.stop_local();
        }
        return conn.close();
    }
    let protocol = hello["hello"]["protocol"].as_u64();
    if protocol != Some(PROTOCOL as u64) {
        conn.send(json!({"refused": {"protocol": PROTOCOL, "error": format!("this daemon speaks protocol {PROTOCOL}, not {}", protocol.map_or("none".to_string(), |p| p.to_string()))}}));
        conn.flush();
        return conn.close();
    }
    conn.send(json!({"welcome": {"protocol": PROTOCOL, "pid": std::process::id(), "build": crate::build_id(), "port": daemon.port(), "instance": daemon.instance()}}));
    for line in lines {
        let Ok(line) = line else { break };
        if !conn.alive.load(Ordering::SeqCst) {
            break;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        let Some(id) = message["id"].as_u64() else { continue };
        if let Some(kind) = message["subscribe"].as_str() {
            subscribe(daemon, &conn, id, kind, &message);
        } else if let Some(sub) = message["unsubscribe"].as_u64() {
            if let Some(flag) = conn.subs.lock().expect("subscriptions").remove(&sub) {
                flag.store(false, Ordering::SeqCst);
            }
            conn.send(json!({"id": id, "status": 200, "reply": {"ok": true}}));
        } else {
            let (daemon, conn) = (daemon.clone(), conn.clone());
            std::thread::spawn(move || {
                let text = |key: &str| message[key].as_str().unwrap_or_default().to_string();
                let body = match &message["body"] {
                    Value::String(text) => text.clone(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                };
                let (status, reply) = daemon.route(&text("method"), &text("path"), &text("query"), &body, true);
                conn.send(json!({"id": id, "status": status, "reply": reply}));
                // The reply to a stop goes out before every connection closes.
                if daemon.stopping() {
                    conn.flush();
                    daemon.stop_local();
                }
            });
        }
    }
    conn.close();
    for (_, flag) in conn.subs.lock().expect("subscriptions").drain() {
        flag.store(false, Ordering::SeqCst);
    }
}

/// Pushes a window's events to the connection as they come, from the same queues HTTP polls read.
fn subscribe(daemon: &Arc<Daemon>, conn: &Arc<Conn>, id: u64, kind: &str, message: &Value) {
    let Some(window) = message["window"].as_str().map(str::to_string) else {
        conn.send(json!({"id": id, "status": 400, "reply": {"error": "window is required"}}));
        return;
    };
    let path = match kind {
        "events" => "/events",
        "conversations" => "/conversations/events",
        _ => {
            conn.send(json!({"id": id, "status": 400, "reply": {"error": format!("no subscription {kind:?}")}}));
            return;
        }
    };
    let flag = Arc::new(AtomicBool::new(true));
    conn.subs.lock().expect("subscriptions").insert(id, flag.clone());
    let mut after = message["after"].as_u64().unwrap_or(0);
    let (daemon, conn) = (daemon.clone(), conn.clone());
    std::thread::spawn(move || {
        while flag.load(Ordering::SeqCst) && conn.alive.load(Ordering::SeqCst) && !daemon.stopping() {
            let (status, reply) = daemon.route("GET", path, &format!("window={window}&after={after}"), "", true);
            // A daemon that stops ends no subscription: the client subscribes again in the next one.
            if !flag.load(Ordering::SeqCst) || daemon.stopping() {
                break;
            }
            if status != 200 {
                conn.send(json!({"id": id, "status": status, "reply": reply}));
                break;
            }
            let events = reply["events"].as_array().cloned().unwrap_or_default();
            if events.is_empty() {
                continue;
            }
            after = events.iter().filter_map(|e| e["seq"].as_u64()).max().unwrap_or(after).max(after);
            if !conn.send(json!({"id": id, "events": events})) {
                break;
            }
        }
    });
}
