//! A callback URL of each session's own (CON-164), over real HTTP.

use harness_daemon::client::Client;
use harness_daemon::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Host {
    open: Mutex<Vec<String>>,
    got: Mutex<Vec<(String, String, String)>>,
}

impl ConversationHost for Host {
    fn op(&self, _: &str, _: &Value) -> Result<Value, String> {
        Ok(Value::Null)
    }
    fn events(&self, _: &str, _: u64, _: Duration) -> Vec<Value> {
        Vec::new()
    }
    fn stats(&self) -> Value {
        Value::Null
    }
    fn close(&self, _: bool) -> bool {
        true
    }
    fn deliver_to(&self, session: &str, label: &str, text: &str) -> bool {
        let open = self.open.lock().unwrap().iter().any(|s| s == session);
        if open {
            self.got.lock().unwrap().push((session.to_string(), label.to_string(), text.to_string()));
        }
        open
    }
    fn session_of(&self, _: &str, tab: u64) -> Option<String> {
        Some(format!("s{tab}"))
    }
}

fn post(url: &str, body: &Value) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent.post(url).send(body.to_string()).unwrap();
    let status = r.status().as_u16();
    (status, r.body_mut().read_json().unwrap_or(Value::Null))
}

fn note(uuid: &str, text: &str) -> Value {
    json!({"uuid": uuid, "type": "notification", "label": "tt", "text": text})
}

struct Fixture {
    data: tempfile::TempDir,
    root: tempfile::TempDir,
    _daemon: Arc<Daemon>,
    client: Client,
    host: Arc<Host>,
}

fn fixture() -> Fixture {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let host = Arc::new(Host::default());
    daemon.host_conversations(host.clone());
    let client = Client::find(data.path()).unwrap();
    Fixture { data, root, _daemon: daemon, client, host }
}

impl Fixture {
    fn root(&self) -> String {
        self.root.path().to_string_lossy().into_owned()
    }
}

/// An open session gets what comes to its URL with its label; the URL stays its own.
#[test]
fn a_session_url_reaches_that_session() {
    let f = fixture();
    f.host.open.lock().unwrap().push("s1".into());
    let url = f.client.session_callback("s1", &f.root()).unwrap();
    assert_eq!(f.client.session_callback("s1", &f.root()).unwrap(), url, "the same URL each time");
    assert_ne!(f.client.session_callback("s2", &f.root()).unwrap(), url, "each session its own");
    // The tool's way: the tab's session, through the conversations route.
    let by_tool = f.client.conversation_op(&json!({"window": "w", "op": "callback_url", "tab": 1, "root": f.root()})).unwrap();
    assert_eq!((by_tool["url"].as_str(), by_tool["session"].as_str()), (Some(url.as_str()), Some("s1")));

    let (status, reply) = post(&url, &note("u1", "filled 0.5 BTC"));
    assert_eq!((status, reply["result"].as_str()), (200, Some("handled")), "{reply}");
    assert_eq!(*f.host.got.lock().unwrap(), [("s1".to_string(), "tt".to_string(), "filled 0.5 BTC".to_string())]);
    // INV-14: once.
    assert_eq!(post(&url, &note("u1", "filled 0.5 BTC")).1["result"], "duplicate");
    // The channel's label is the operator's alone (INV-12).
    post(&url, &json!({"uuid": "u2", "type": "notification", "label": "telegram", "text": "x"}));
    assert_eq!(f.host.got.lock().unwrap()[1].1, "callback");
    // Step-debug is a tab's.
    let (status, reply) = post(&url, &json!({"uuid": "u3", "type": "flowstate_update", "map": 1, "node": 1, "instance": "i"}));
    assert_eq!(status, 400, "{reply}");
    // An unknown token.
    let base = url.rsplit_once('/').unwrap().0;
    assert_eq!(post(&format!("{base}/no-such-token"), &note("u4", "x")).0, 401);
    // The token never goes to the log.
    let token = url.rsplit('/').next().unwrap();
    let log = std::fs::read_to_string(f.data.path().join("daemon/triggers.jsonl")).unwrap();
    assert!(!log.contains(token) && log.contains("session:s1"), "{log}");
}

/// A session not open keeps what comes, in order, and gets it when it opens; a restart keeps both
/// the URL and what waits.
#[test]
fn a_session_not_open_gets_it_later() {
    let f = fixture();
    let url = f.client.session_callback("s7", &f.root()).unwrap();
    for (uuid, text) in [("q1", "first"), ("q2", "second")] {
        let (status, reply) = post(&url, &note(uuid, text));
        assert_eq!((status, reply["result"].as_str()), (200, Some("queued")), "{reply}");
    }
    assert!(f.host.got.lock().unwrap().is_empty());

    // A restart: the same URL, the same waiting.
    f.client.shutdown().unwrap();
    let second = Daemon::start(f.data.path(), 0).unwrap();
    second.host_conversations(f.host.clone());
    let client = Client::find(f.data.path()).unwrap();
    let again = client.session_callback("s7", &f.root()).unwrap();
    assert_eq!(again.rsplit('/').next(), url.rsplit('/').next(), "the same token");
    let url = again;

    f.host.open.lock().unwrap().push("s7".into());
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while f.host.got.lock().unwrap().len() < 2 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let texts: Vec<String> = f.host.got.lock().unwrap().iter().map(|(_, _, t)| t.clone()).collect();
    assert_eq!(texts, ["first", "second"], "in order, once it opened");
    // Now open: at once.
    assert_eq!(post(&url, &note("q3", "third")).1["result"], "handled");
    assert_eq!(f.host.got.lock().unwrap().len(), 3);
}
