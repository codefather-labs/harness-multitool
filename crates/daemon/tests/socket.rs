//! The local socket of the daemon (CON-130, ADR 0027), over a real socket.

use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use harness_daemon::socket::{PROTOCOL, connect, socket_path};
use harness_daemon::{Address, Daemon};
use serde_json::{Value, json};

/// A bare client of the protocol: one line out, lines in.
struct Raw {
    out: socket2::Socket,
    lines: std::io::Lines<BufReader<socket2::Socket>>,
}

impl Raw {
    fn open(data: &std::path::Path, protocol: u32) -> (Raw, Value) {
        let socket = connect(&socket_path(data)).expect("the daemon listens");
        let mut raw = Raw { out: socket.try_clone().unwrap(), lines: BufReader::new(socket).lines() };
        raw.send(json!({"hello": {"protocol": protocol, "build": "test", "role": "window"}}));
        let first = raw.next();
        (raw, first)
    }

    fn send(&mut self, message: Value) {
        self.out.write_all(format!("{message}\n").as_bytes()).unwrap();
    }

    fn next(&mut self) -> Value {
        serde_json::from_str(&self.lines.next().expect("a line").expect("readable")).unwrap()
    }

    /// The reply with this id, skipping pushes.
    fn reply(&mut self, id: u64) -> Value {
        loop {
            let message = self.next();
            if message["id"] == id && message.get("status").is_some() {
                return message;
            }
        }
    }

    fn request(&mut self, id: u64, method: &str, path: &str, body: Value) -> Value {
        self.send(json!({"id": id, "method": method, "path": path, "query": "", "body": body.to_string()}));
        self.reply(id)
    }
}

fn daemon() -> (tempfile::TempDir, std::sync::Arc<Daemon>) {
    let data = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    (data, daemon)
}

fn notify(port: u16, window: &str, uuid: &str) -> u16 {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let body = json!({"uuid": uuid, "type": "notification", "label": "t", "text": uuid}).to_string();
    agent.post(format!("http://127.0.0.1:{port}/callback/{window}")).send(body).unwrap().status().as_u16()
}

#[test]
fn a_client_of_the_same_protocol_is_welcomed_and_another_refused() {
    let (data, _daemon) = daemon();
    let (_ok, welcome) = Raw::open(data.path(), PROTOCOL);
    assert_eq!(welcome["welcome"]["protocol"], PROTOCOL);
    assert_eq!(welcome["welcome"]["pid"], std::process::id());
    let (mut other, refused) = Raw::open(data.path(), PROTOCOL + 1);
    assert_eq!(refused["refused"]["protocol"], PROTOCOL, "{refused}");
    assert!(other.lines.next().is_none(), "the connection closes after the refusal");
}

#[test]
fn the_socket_is_named_in_the_address_and_only_its_user_can_open_it() {
    let (data, _daemon) = daemon();
    let path = socket_path(data.path());
    assert_eq!(path, data.path().join("daemon").join("daemon.sock"));
    let address: Address = serde_json::from_str(&std::fs::read_to_string(data.path().join("daemon/address.json")).unwrap()).unwrap();
    assert_eq!(address.socket.as_deref(), Some(path.as_path()));
    assert_eq!(address.protocol, PROTOCOL);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

/// Thirty clients at once, one of them subscribed and never reading while its events pile up: the
/// others are answered and pushed to at once.
#[test]
fn a_client_that_never_reads_holds_up_no_other() {
    let (data, daemon) = daemon();
    let root = tempfile::tempdir().unwrap();
    let root = root.path().to_string_lossy().into_owned();
    // The stuck one: subscribed to a busy window, never reading.
    let (mut stuck, _) = Raw::open(data.path(), PROTOCOL);
    let busy = stuck.request(1, "POST", "/register", json!({"root": root}))["reply"]["window"].as_str().unwrap().to_string();
    stuck.send(json!({"id": 2, "subscribe": "events", "window": busy, "after": 0}));
    let flood = std::thread::spawn({
        let (port, busy) = (daemon.port(), busy.clone());
        move || (0..3000).for_each(|n| {
            notify(port, &busy, &format!("flood-{n}"));
        })
    });
    let mut clients = Vec::new();
    for n in 0..29 {
        let (mut c, _) = Raw::open(data.path(), PROTOCOL);
        let window = c.request(1, "POST", "/register", json!({"root": root}))["reply"]["window"].as_str().unwrap().to_string();
        c.send(json!({"id": 2, "subscribe": "events", "window": window, "after": 0}));
        clients.push((n, c, window));
    }
    std::thread::sleep(Duration::from_millis(300));
    let mut slowest = (Duration::ZERO, Duration::ZERO);
    for (n, c, window) in clients.iter_mut() {
        let start = Instant::now();
        let health = c.request(10, "GET", "/health", Value::Null);
        assert_eq!(health["status"], 200);
        slowest.0 = slowest.0.max(start.elapsed());
        let start = Instant::now();
        assert_eq!(notify(daemon.port(), window, &format!("mine-{n}")), 200);
        let pushed = loop {
            let message = c.next();
            if message["id"] == 2 {
                break message;
            }
        };
        slowest.1 = slowest.1.max(start.elapsed());
        assert_eq!(pushed["events"][0]["event"]["uuid"], format!("mine-{n}"), "a window hears only its own events");
    }
    assert!(slowest.0 < Duration::from_millis(50), "a request took {:?}", slowest.0);
    assert!(slowest.1 < Duration::from_millis(50), "a push took {:?}", slowest.1);
    flood.join().unwrap();
    drop(stuck);
}

#[test]
fn events_of_one_window_never_reach_another() {
    let (data, daemon) = daemon();
    let root = tempfile::tempdir().unwrap();
    let root = root.path().to_string_lossy().into_owned();
    let (mut a, _) = Raw::open(data.path(), PROTOCOL);
    let (mut b, _) = Raw::open(data.path(), PROTOCOL);
    let wa = a.request(1, "POST", "/register", json!({"root": root}))["reply"]["window"].as_str().unwrap().to_string();
    let wb = b.request(1, "POST", "/register", json!({"root": root}))["reply"]["window"].as_str().unwrap().to_string();
    a.send(json!({"id": 2, "subscribe": "events", "window": wa, "after": 0}));
    b.send(json!({"id": 2, "subscribe": "events", "window": wb, "after": 0}));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(notify(daemon.port(), &wa, "for-a"), 200);
    assert_eq!(notify(daemon.port(), &wb, "for-b"), 200);
    assert_eq!(a.next()["events"][0]["event"]["uuid"], "for-a");
    assert_eq!(b.next()["events"][0]["event"]["uuid"], "for-b");
    // A reply goes to the connection that asked, under its id.
    let ra = a.request(3, "GET", "/stats", Value::Null);
    assert_eq!(ra["id"], 3);
}

#[test]
fn a_stopped_daemon_closes_every_connection_and_removes_its_socket() {
    let (data, daemon) = daemon();
    let (mut a, _) = Raw::open(data.path(), PROTOCOL);
    let (mut b, _) = Raw::open(data.path(), PROTOCOL);
    let stop = a.request(1, "POST", "/shutdown", json!({}));
    assert_eq!(stop["status"], 200);
    let start = Instant::now();
    while b.lines.next().is_some_and(|l| l.is_ok()) {}
    assert!(start.elapsed() < Duration::from_secs(1), "the other client saw the loss at once");
    let start = Instant::now();
    while socket_path(data.path()).exists() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!socket_path(data.path()).exists());
    assert!(connect(&socket_path(data.path())).is_err());
    assert!(daemon.stopping());
}

#[test]
fn a_second_daemon_on_the_same_data_refuses_to_start() {
    let (data, _daemon) = daemon();
    let error = Daemon::start(data.path(), 0).err().expect("refused");
    assert!(error.contains("already listens"), "{error}");
}
