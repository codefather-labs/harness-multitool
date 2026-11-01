//! Step-debug: saving a run at the end of a cycle, and the conformance check of a debug router.

use std::sync::{Arc, Mutex};

use harness_api::{MapKind, Memory, NewMapNode};
use harness_daemon::client::Client;
use harness_daemon::stepdebug::*;
use harness_daemon::*;
use harness_memory_journal::plugin::JournalMemory;
use harness_memory_journal::project::key_for;
use serde_json::{Value, json};
use std::path::Path;

fn now<T>(mut future: harness_api::BoxFuture<'_, T>) -> T {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("should be ready"),
    }
}

struct World {
    _data: tempfile::TempDir,
    root: tempfile::TempDir,
    _daemon: Arc<Daemon>,
    client: Client,
    window: String,
    url: String,
    map: u64,
    nodes: Vec<u64>,
}

fn world() -> World {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let reg = client.register(&root_path).unwrap();
    let store = JournalMemory::open(&data.path().join("journals"), &key_for(None, Path::new(&root_path)), "t").unwrap();
    let map = now(store.map_new(MapKind::Flowchart, "pipeline", None)).unwrap().id;
    let mut last = None;
    let mut nodes = Vec::new();
    for (kind, text) in [("process", "fetch"), ("end", "done")] {
        let data = now(store.map_add(map, NewMapNode { text: text.into(), kind: kind.into(), after: last, ..Default::default() })).unwrap();
        let id = data.nodes.iter().find(|n| n.text == text).unwrap().id;
        nodes.push(id);
        last = Some(id);
    }
    World { _data: data, root, _daemon: daemon, client, window: reg.window, url: reg.url, map, nodes }
}

fn flow(w: &World, uuid: &str, node: u64, data: Value) -> Value {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let body = json!({"uuid": uuid, "type": "flowstate_update", "map": w.map, "node": node, "instance": "req-1", "state": "entered", "data": data});
    agent.post(&w.url).send(body.to_string()).unwrap().body_mut().read_json().unwrap()
}

#[test]
fn a_window_in_the_mode_saves_each_cycle_and_starts_over() {
    let w = world();
    let root = w.root.path().to_string_lossy().into_owned();
    w.client.set_step_debug(&w.window, true).unwrap();
    flow(&w, "a", w.nodes[0], json!({"rows": 3}));
    flow(&w, "b", 999, json!({}));
    assert!(runs(&root, 10).unwrap().is_empty(), "an off-chart event does not end it");
    let answer = flow(&w, "c", w.nodes[1], json!({"ok": true}));
    assert_eq!(answer["detail"]["run"], 1, "{answer}");

    let saved = runs(&root, 10).unwrap();
    assert_eq!(saved.len(), 1);
    let steps = &saved[0].steps;
    assert_eq!(steps.iter().map(|s| (s.node, s.off_chart)).collect::<Vec<_>>(), [(w.nodes[0], false), (999, true), (w.nodes[1], false)], "INV-13: in order, off-chart too");
    assert_eq!(steps[0].data, json!({"rows": 3}));
    assert!(steps[0].at_ms > 0 && steps[0].at_ms <= steps[2].at_ms);

    // The next pass starts over and is a run of its own.
    flow(&w, "d", w.nodes[0], json!({}));
    let events = w.client.poll(&w.window, 0).unwrap();
    let Some(WindowEvent::Flow { path, .. }) = events.last().map(|e| &e.event) else { panic!("{events:?}") };
    assert_eq!(path, &[w.nodes[0]], "the path was reset");
    flow(&w, "e", w.nodes[1], json!({}));
    let saved = runs(&root, 10).unwrap();
    assert_eq!((saved.len(), saved[0].steps.len()), (2, 2), "newest first");
}

#[test]
fn a_window_not_in_the_mode_creates_nothing_whatever_the_payload_says() {
    let w = world();
    let answer = flow(&w, "a", w.nodes[1], json!({"step_debug": true, "save": true}));
    assert_eq!(answer["result"], "handled", "{answer}");
    assert_eq!(answer["detail"]["run"], Value::Null, "{answer}");
    assert!(!w.root.path().join(".harness").exists(), "no folder appears in the project");
    assert!(runs(&w.root.path().to_string_lossy(), 10).unwrap().is_empty());
    assert!(!w.root.path().join(".harness").exists(), "reading creates nothing either");

    w.client.set_step_debug(&w.window, true).unwrap();
    w.client.set_step_debug(&w.window, false).unwrap();
    flow(&w, "b", w.nodes[1], json!({}));
    assert!(!w.root.path().join(".harness").exists(), "switched off again");
}

#[test]
fn a_restart_keeps_a_windows_mode() {
    let w = world();
    w.client.set_step_debug(&w.window, true).unwrap();
    let again = Daemon::start(w._data.path(), 0).unwrap();
    let client = Client::find(w._data.path()).unwrap();
    assert_eq!(client.stats().unwrap()["step_debug_windows"], 1);
    drop(again);
}

type Routes = Arc<Mutex<Vec<String>>>;

/// A debug router: `routes` maps "METHOD path" to its answer without and with a token.
fn router(answers: &'static [(&'static str, u16, u16)]) -> (String, Routes) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", server.server_addr().to_ip().unwrap().port());
    let seen: Routes = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        while let Ok(request) = server.recv() {
            let key = format!("{} {}", request.method(), request.url());
            let token = request.headers().iter().any(|h| h.field.equiv("authorization") && h.value.as_str() == "Bearer t");
            log.lock().unwrap().push(format!("{key} {token}"));
            let code = answers.iter().find(|(k, ..)| *k == key).map_or(404, |(_, no, yes)| if token { *yes } else { *no });
            request.respond(tiny_http::Response::from_string("{}").with_status_code(code)).unwrap();
        }
    });
    (url, seen)
}

fn failures(report: &[Check]) -> Vec<String> {
    report.iter().filter(|c| !c.ok).map(|c| c.line.clone()).collect()
}

const STATUS: (&str, u16, u16) = ("GET /debug/status", 401, 200);
const APPROVE: (&str, u16, u16) = ("POST /debug/approve", 401, 200);

#[test]
fn a_conforming_router_passes_and_is_not_moved_on() {
    let (url, seen) = router(&[STATUS, APPROVE]);
    let report = check_router(&url, Some("t"), false);
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(report.iter().any(|c| c.line.contains("absent, as it must be")), "{report:?}");
    assert!(!seen.lock().unwrap().iter().any(|l| l == "POST /debug/approve true"), "no approve with a token unless asked");
    assert!(!seen.lock().unwrap().iter().any(|l| l == "POST /debug/step-back true"));

    let (url, seen) = router(&[STATUS, APPROVE, ("POST /debug/step-back", 401, 200)]);
    let report = check_router(&url, Some("t"), true);
    assert_eq!(failures(&report), Vec::<String>::new());
    assert!(report.iter().any(|c| c.line.contains("step-back: present")), "{report:?}");
    assert!(seen.lock().unwrap().iter().any(|l| l == "POST /debug/approve true"), "asked for");
}

#[test]
fn a_router_breaking_the_contract_fails() {
    let (url, _) = router(&[("GET /debug/status", 200, 200), APPROVE]);
    assert!(failures(&check_router(&url, Some("t"), false)).iter().any(|f| f.contains("with no token")));
    let (url, _) = router(&[STATUS, APPROVE, ("POST /debug/step-back", 501, 501)]);
    assert!(failures(&check_router(&url, Some("t"), false)).iter().any(|f| f.contains("stub")));
    let (url, _) = router(&[STATUS]);
    assert!(failures(&check_router(&url, Some("t"), false)).iter().any(|f| f.contains("approve: missing")));
    let (url, _) = router(&[("GET /debug/status", 401, 401), APPROVE]);
    assert!(failures(&check_router(&url, Some("t"), false)).iter().any(|f| f.contains("with the token")));
}

#[test]
fn the_contract_names_its_routes() {
    for route in ["/debug/status", "/debug/approve", "/debug/step-back", "bearer", "MULTITOOL_DEBUG_MODE"] {
        assert!(CONTRACT.contains(route), "{route}");
    }
}
