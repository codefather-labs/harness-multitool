//! harness-tools-mcp against a fake stdio server (tests/fake_server.py), CON-201.
#![cfg(unix)]

use harness_api::Tool;
use harness_tools_mcp::client::{Client, Failure};
use harness_tools_mcp::discovery::Server;
use harness_tools_mcp::{McpTool, Start};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

fn script() -> String {
    format!("{}/tests/fake_server.py", env!("CARGO_MANIFEST_DIR"))
}

fn fake(name: &str, env: &[(&str, &str)]) -> Server {
    Server { name: name.into(), source: "test".into(), command: "python3".into(), args: vec![script()], env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
}

#[test]
fn a_client_initializes_lists_every_page_and_calls() {
    let root = tempfile::tempdir().unwrap();
    let mut c = Client::start("python3", &[script()], &BTreeMap::new(), None, &root.path().to_string_lossy(), None, Duration::from_secs(20)).unwrap();
    let tools = c.list(Duration::from_secs(10)).unwrap();
    assert_eq!(tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["echo", "fail", "sleep", "die", "roots"], "both pages");
    assert_eq!(tools[0].schema["required"], json!(["text"]));
    let echo = c.call("echo", &json!({"text": "hi"}), Duration::from_secs(10)).unwrap();
    assert_eq!((echo.text.as_str(), echo.is_error), ("echo: hi\n[image image/png]", false));
    let fail = c.call("fail", &json!({}), Duration::from_secs(10)).unwrap();
    assert_eq!((fail.text.as_str(), fail.is_error), ("it failed", true));
    assert!(matches!(c.call("nope", &json!({}), Duration::from_secs(10)), Err(Failure::Refused(m)) if m == "no tool nope"));
    let roots = c.call("roots", &json!({}), Duration::from_secs(10)).unwrap();
    assert!(roots.text.contains(&format!("file://{}", root.path().display())), "the server asked and got the project: {}", roots.text);
    assert!(matches!(c.call("sleep", &json!({}), Duration::from_millis(500)), Err(Failure::TimedOut(_))));
    assert!(matches!(c.call("die", &json!({}), Duration::from_secs(10)), Err(Failure::Gone(_))));
    // Its stdout closes as it exits, a moment before the process is gone.
    let until = std::time::Instant::now() + Duration::from_secs(5);
    while c.alive() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!c.alive());
}

#[test]
fn the_hub_lists_once_keeps_the_list_and_starts_a_gone_server_again() {
    let t = tempfile::tempdir().unwrap();
    let starts = t.path().join("starts");
    let server = fake("fake", &[("FAKE_STARTS", &starts.to_string_lossy())]);
    let start = Start { base: None, root: t.path().to_string_lossy().into_owned(), dir: t.path().join("mcp") };
    assert_eq!(harness_tools_mcp::cached(&server, &start), None);
    let tools = harness_tools_mcp::tools(&server, &start, Duration::from_secs(30)).unwrap().expect("listed in time");
    assert_eq!(tools.len(), 5);
    assert_eq!(harness_tools_mcp::cached(&server, &start).unwrap().len(), 5, "kept on disk");
    let echo = McpTool { server: Arc::new(server.clone()), tool: tools[0].clone(), start: start.clone() };
    assert_eq!(echo.spec().name, "mcp__fake__echo");
    assert_eq!(echo.capabilities(&json!({})).unwrap(), [harness_api::Capability::Command("mcp:fake/echo".into())]);
    let called = harness_tools_mcp::call(&server, &start, "echo", &json!({"text": "a"})).unwrap();
    assert_eq!(called.text, "echo: a\n[image image/png]");
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 1, "the listing's server answered the call");
    // It dies; the next call starts it again.
    assert!(harness_tools_mcp::call(&server, &start, "die", &json!({})).is_err());
    assert_eq!(harness_tools_mcp::call(&server, &start, "echo", &json!({"text": "b"})).unwrap().text, "echo: b\n[image image/png]");
    assert_eq!(std::fs::read_to_string(&starts).unwrap().lines().count(), 2);
    let log = std::fs::read_dir(t.path().join("mcp")).unwrap().filter_map(|e| e.ok()).find(|e| e.path().extension().is_some_and(|x| x == "log")).unwrap();
    assert!(std::fs::read_to_string(log.path()).unwrap().contains("call echo"), "stderr goes to the log");
}

#[test]
fn a_server_that_does_not_start_says_so_and_one_slow_to_list_comes_later() {
    let t = tempfile::tempdir().unwrap();
    let start = Start { base: None, root: t.path().to_string_lossy().into_owned(), dir: t.path().join("mcp") };
    let missing = Server { name: "missing".into(), source: "test".into(), command: "/no/such/program".into(), args: vec![], env: BTreeMap::new() };
    assert!(harness_tools_mcp::tools(&missing, &start, Duration::from_secs(5)).unwrap_err().contains("/no/such/program"));
    let slow = Server { name: "slow".into(), source: "test".into(), command: "sh".into(), args: vec!["-c".into(), format!("sleep 1; exec python3 {}", script())], env: BTreeMap::new() };
    assert_eq!(harness_tools_mcp::tools(&slow, &start, Duration::from_millis(100)).unwrap(), None, "not listed in time");
    let until = std::time::Instant::now() + Duration::from_secs(20);
    while harness_tools_mcp::cached(&slow, &start).is_none() && std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(harness_tools_mcp::cached(&slow, &start).map(|t| t.len()), Some(5), "listed in the background");
}
