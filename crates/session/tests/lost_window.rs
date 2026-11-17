//! CON-181: a window lost with a run on. Its own test binary, so the shortened time a window may be
//! silent (MULTITOOL_WINDOW_GONE_SECS) is this process's only.

use std::sync::Arc;
use std::time::Duration;

use harness_api::*;
use harness_session::*;
use serde_json::json;

mod common;
use common::*;

/// CON-181: a lost window (silent past its time) leaves as it said on opening; by default its
/// work finishes. MULTITOOL_WINDOW_GONE_SECS shortens the time for the test.
#[test]
fn a_lost_window_finishes_its_work_by_default() {
    // SAFETY: set before any thread of this test binary reads the environment; this binary holds
    // this one test.
    unsafe { std::env::set_var("MULTITOOL_WINDOW_GONE_SECS", "2") };
    use harness_session::remote::{DaemonHost, MakeHost};
    let root = temp_dir("lost-window");
    let data = temp_dir("lost-window-data");
    let (url, seen) = fake_endpoint(vec![anthropic_turn(Some(("w1", "shell", json!({"command": "sleep 4; echo slept"}))), "Working."), anthropic_turn(None, "DONE")]);
    let daemon = harness_daemon::Daemon::start(std::path::Path::new(&data), 0).unwrap();
    let make_host: MakeHost = Arc::new(|_, note, reload| Host { note, plugins: Arc::new(|| json!({})), reload, environment: None, window: None, choices: Default::default() });
    daemon.host_conversations(DaemonHost::new(std::path::Path::new(&data), make_host));
    let client = harness_daemon::client::Client::find(std::path::Path::new(&data)).unwrap();
    let op = |op: serde_json::Value| {
        let mut op = op;
        op["window"] = json!("conv-888-lost");
        client.conversation_op(&op).unwrap()
    };
    op(json!({"op": "open", "root": root, "settings": settings(&url, "k")}));
    op(json!({"op": "prompt", "tab": 1, "text": "work"}));
    let (mut after, deadline) = (0, std::time::Instant::now() + Duration::from_secs(10));
    let mut asked = false;
    while !asked && std::time::Instant::now() < deadline {
        for event in client.conversation_events("conv-888-lost", after).unwrap() {
            after = event["seq"].as_u64().unwrap();
            asked |= event["event"]["kind"] == "ask";
        }
    }
    let session = op(json!({"op": "session_of", "tab": 1}))["session"].as_str().unwrap().to_string();
    op(json!({"op": "answer", "tab": 1, "answer": "once"}));
    // Lost: no poll, no op, no close, as a killed window or a dropped ssh leaves it.
    std::thread::sleep(Duration::from_secs(9));
    assert_eq!(seen.lock().unwrap().len(), 2, "the run went on to its end");
    assert!(read_log(&data, &root, &session).iter().any(|e| matches!(&e.event, Event::TextDelta { text } if text.contains("DONE"))));
}

