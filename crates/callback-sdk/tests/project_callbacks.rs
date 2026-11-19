//! CON-183: custom callbacks as project handlers, over real HTTP. A project with handlers under
//! .harness/callbacks, Rust programs written on this SDK (journal#1373); a conversation registered
//! for one gets what its handler sends back; another, not registered, gets nothing; a script is
//! refused.
#![cfg(unix)]

use harness_daemon::client::Client;
use harness_daemon::*;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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

/// A callback folder whose handler is a copy of one of this crate's Rust programs (journal#1373).
fn handler(root: &std::path::Path, name: &str, config: &str, program: &str) {
    let folder = root.join(".harness/callbacks").join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.toml"), format!("handler = \"handler\"\n{config}")).unwrap();
    std::fs::copy(program, folder.join("handler")).unwrap();
    std::fs::set_permissions(folder.join("handler"), std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn wait(what: impl Fn() -> bool, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end && !what() {
        std::thread::sleep(Duration::from_millis(100));
    }
    what()
}

#[test]
fn a_project_handler_stands_between_the_daemon_and_the_conversation() {
    let (data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = project.path().to_string_lossy().into_owned();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let host = Arc::new(Host::default());
    daemon.host_conversations(host.clone());
    host.open.lock().unwrap().extend(["s1".to_string(), "s2".to_string()]);
    let client = Client::find(data.path()).unwrap();
    handler(project.path(), "echo", "description = \"says it back\"\ntypes = [\"ping\"]\nargs = [\"--from-config\"]\n", env!("CARGO_BIN_EXE_test-echo"));
    handler(project.path(), "slow", "types = [\"slow\"]\ntimeout_s = 1\n", env!("CARGO_BIN_EXE_test-slow"));
    handler(project.path(), "liar", "types = [\"liar\"]\ntimeout_s = 1\n", env!("CARGO_BIN_EXE_test-liar"));
    let op = |tab: u64, op: &str, extra: Value| {
        let mut body = json!({"window": "w1", "tab": tab, "root": root, "op": op});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        client.conversation_op(&body).unwrap()
    };
    let (url1, url2) = (client.session_callback("s1", &root).unwrap(), client.session_callback("s2", &root).unwrap());
    let ping = |uuid: &str, url: &str| post(url, &json!({"uuid": uuid, "type": "ping", "payload": {"n": 7}}));
    let marker = project.path().join("echo-stdin.json");

    // callback_url tells an agent how project callbacks work (CON-183).
    let usage = op(1, "callback_url", json!({}))["usage"].as_str().unwrap_or_default().to_string();
    for part in ["harness-callback", "multitool callback new", "Settings > Callbacks", "callback_handlers", "docs/callbacks.md"] {
        assert!(usage.contains(part), "{part}: {usage}");
    }
    // Listed, not approved, not registered: it does not run.
    let listed = op(1, "project_callbacks", json!({}));
    let echo = listed["callbacks"].as_array().unwrap().iter().find(|c| c["name"] == "echo").unwrap().clone();
    assert_eq!((echo["standing"].as_str(), echo["subscribed"].as_str()), (Some("unapproved"), Some("no")), "{listed}");
    assert_eq!(listed["url"].as_str(), Some(url1.as_str()), "the conversation's URL with it");
    let (code, said) = ping("u1", &url1);
    assert_eq!(code, 400, "not registered for s1 yet: {said}");
    assert!(said["error"].as_str().unwrap().contains("not subscribed"), "{said}");
    // Registered, still not approved: 409, and nothing ran.
    op(1, "callback_subscribe", json!({"name": "echo", "on": true}));
    let (code, said) = ping("u2", &url1);
    assert_eq!(code, 409, "{said}");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!marker.exists(), "an unapproved handler never runs");
    // Approved: 202 at once, and the result reaches s1 alone.
    op(1, "callback_approve", json!({"name": "echo"}));
    let started = Instant::now();
    let (code, said) = ping("u3", &url1);
    assert_eq!((code, said["result"].as_str()), (202, Some("started")), "{said}");
    assert!(started.elapsed() < Duration::from_secs(2), "the caller does not wait for the handler");
    assert!(wait(|| host.got.lock().unwrap().iter().any(|(s, _, t)| s == "s1" && t == "echo ping for s1"), 10), "{:?}", host.got.lock().unwrap());
    assert!(host.got.lock().unwrap().iter().all(|(s, _, _)| s == "s1"), "nothing for s2");
    let stdin: Value = serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
    assert_eq!((stdin["uuid"].as_str(), stdin["payload"]["n"].as_u64()), (Some("u3"), Some(7)), "the callback on stdin");
    assert_eq!(std::fs::read_to_string(project.path().join("echo-sessions.txt")).unwrap(), "s1");
    assert_eq!(std::fs::read_to_string(project.path().join("echo-args.txt")).unwrap(), "--from-config");
    assert_eq!(std::fs::read_to_string(project.path().join("echo-reply.txt")).unwrap(), "delivered", "the SDK's reply");
    let label = host.got.lock().unwrap().iter().find(|(s, _, _)| s == "s1").map(|(_, l, _)| l.clone()).unwrap();
    assert_eq!(label, "echo", "the label is the callback's name");
    // The same uuid: nothing runs again.
    let runs = host.got.lock().unwrap().len();
    let (code, said) = ping("u3", &url1);
    assert_eq!((code, said["result"].as_str()), (200, Some("duplicate")));
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(host.got.lock().unwrap().len(), runs);
    // s2, not registered: 400.
    assert_eq!(ping("u4", &url2).0, 400);
    // The handler changed since it was approved: 409 until approved again.
    let echo_file = project.path().join(".harness/callbacks/echo/handler");
    let mut bytes = std::fs::read(&echo_file).unwrap();
    bytes.push(0);
    std::fs::write(&echo_file, bytes).unwrap();
    let (code, said) = ping("u5", &url1);
    assert_eq!(code, 409, "{said}");
    assert!(said["error"].as_str().unwrap().contains("changed"), "{said}");
    assert_eq!(op(1, "project_callbacks", json!({}))["callbacks"].as_array().unwrap().iter().find(|c| c["name"] == "echo").unwrap()["standing"], "changed");
    op(1, "callback_approve", json!({"name": "echo"}));
    assert_eq!(ping("u6", &url1).0, 202);
    // Past its time: killed, and nothing comes.
    op(1, "callback_subscribe", json!({"name": "slow", "on": true}));
    op(1, "callback_approve", json!({"name": "slow"}));
    let before = host.got.lock().unwrap().len();
    assert_eq!(post(&url1, &json!({"uuid": "u7", "type": "slow"})).0, 202);
    std::thread::sleep(Duration::from_secs(4));
    let log = std::fs::read_to_string(data.path().join("daemon/triggers.jsonl")).unwrap_or_default();
    assert!(log.contains("timed_out"), "the trigger log says so");
    assert_eq!(host.got.lock().unwrap().len(), before, "a killed handler sends nothing");
    assert!(!host.got.lock().unwrap().iter().any(|(_, _, t)| t == "too late"));
    // A result for another conversation is refused; a run past its time gets 410; no run, 404.
    op(1, "callback_subscribe", json!({"name": "liar", "on": true}));
    op(1, "callback_approve", json!({"name": "liar"}));
    assert_eq!(post(&url1, &json!({"uuid": "u8", "type": "liar"})).0, 202);
    let code = project.path().join("liar-code.txt");
    assert!(wait(|| code.exists(), 10));
    assert_eq!(std::fs::read_to_string(&code).unwrap(), "403");
    let reply = std::fs::read_to_string(project.path().join("liar-url.txt")).unwrap();
    assert_eq!(post(&format!("{}x", reply), &json!({"text": "no"})).0, 404);
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(post(&reply, &json!({"text": "late"})).0, 410);
    // A script is not a handler: listed with its problem, and it cannot be approved.
    let folder = project.path().join(".harness/callbacks/script");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("config.toml"), "handler = \"run.sh\"\ntypes = [\"script\"]\n").unwrap();
    std::fs::write(folder.join("run.sh"), "#!/bin/sh\ntouch \"$MULTITOOL_PROJECT/script-ran\"\n").unwrap();
    std::fs::set_permissions(folder.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let listed = op(1, "project_callbacks", json!({}));
    let script = listed["callbacks"].as_array().unwrap().iter().find(|c| c["name"] == "script").unwrap().clone();
    assert!(script.to_string().contains("compiled Rust program"), "{script}");
    let approved = client.conversation_op(&json!({"window": "w1", "tab": 1, "root": root, "op": "callback_approve", "name": "script"}));
    assert!(approved.is_err() || approved.as_ref().unwrap().get("error").is_some(), "{approved:?}");
    op(1, "callback_subscribe", json!({"name": "script", "on": true}));
    assert_eq!(post(&url1, &json!({"uuid": "u10", "type": "script"})).0, 409);
    std::thread::sleep(Duration::from_millis(500));
    assert!(!project.path().join("script-ran").exists(), "a script never runs");
    // Registration off: back to 400.
    op(1, "callback_subscribe", json!({"name": "echo", "on": false}));
    assert_eq!(ping("u9", &url1).0, 400);
    drop(daemon);
}
