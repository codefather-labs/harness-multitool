//! The callback entry over real HTTP (IF-13, IF-16, INV-12, INV-14).

use harness_api::MapKind;
use harness_daemon::client::Client;
use harness_daemon::*;
use harness_memory_journal::plugin::JournalMemory;
use harness_memory_journal::project::key_for;
use serde_json::{Value, json};
use std::path::Path;

struct Fixture {
    _data: tempfile::TempDir,
    _root: tempfile::TempDir,
    daemon: std::sync::Arc<Daemon>,
    client: Client,
    window: String,
    url: String,
    data_path: std::path::PathBuf,
    root_path: String,
}

fn fixture() -> Fixture {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let client = Client::find(data.path()).expect("the daemon published its address");
    let root_path = root.path().to_string_lossy().into_owned();
    let reg = client.register(&root_path).unwrap();
    let data_path = data.path().to_path_buf();
    Fixture { _data: data, _root: root, daemon, client, window: reg.window, url: reg.url, data_path, root_path }
}

fn post(url: &str, body: &str) -> (u16, Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut r = agent.post(url).send(body.to_string()).unwrap();
    let status = r.status().as_u16();
    (status, r.body_mut().read_json().unwrap_or(Value::Null))
}

fn events(f: &Fixture, after: u64) -> Vec<Numbered> {
    f.client.poll(&f.window, after).unwrap()
}

fn log(f: &Fixture) -> Vec<Value> {
    std::fs::read_to_string(f.data_path.join("daemon/triggers.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn a_notification_reaches_its_window_once() {
    let f = fixture();
    let body = json!({"uuid": "u1", "type": "notification", "label": "ci", "text": "build failed"}).to_string();
    assert_eq!(post(&f.url, &body).0, 200);
    let got = events(&f, 0);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].event, WindowEvent::Notification { uuid: "u1".into(), label: "ci".into(), text: "build failed".into() });

    // INV-14: the same uuid again is logged, not delivered.
    let (status, reply) = post(&f.url, &body);
    assert_eq!((status, reply["result"].as_str()), (200, Some("duplicate")));
    assert_eq!(events(&f, 0).len(), 1);
    let outcomes: Vec<_> = log(&f).iter().map(|l| l["outcome"].as_str().unwrap().to_string()).collect();
    assert_eq!(outcomes, ["handled", "duplicate"]);
}

#[test]
fn the_contract_is_enforced() {
    let f = fixture();
    assert_eq!(post(&f.url, "not json").0, 400);
    assert_eq!(post(&f.url, &json!({"type": "notification"}).to_string()).0, 400, "no uuid");
    assert_eq!(post(&f.url, &json!({"uuid": "x"}).to_string()).0, 400, "no type");
    // No handler: 400, logged, reaches no one.
    let (status, reply) = post(&f.url, &json!({"uuid": "u2", "type": "deploy_done"}).to_string());
    assert_eq!(status, 400);
    assert!(reply["error"].as_str().unwrap().contains("deploy_done"));
    assert!(log(&f).iter().any(|l| l["outcome"] == "no_handler" && l["uuid"] == "u2"));
    // An unknown window and a missing admin token are refused.
    let base = format!("http://127.0.0.1:{}", f.daemon.port());
    assert_eq!(post(&format!("{base}/callback/nope"), "{}").0, 401);
    assert_eq!(post(&format!("{base}/register"), "{}").0, 401);
    assert_eq!(post(&format!("{base}/other"), "{}").0, 404);
}

/// INV-12: a payload naming a skill, a session, a tool or an approval changes nothing.
#[test]
fn a_payload_is_only_data() {
    let f = fixture();
    let body = json!({"uuid": "u3", "type": "notification", "text": "hello", "skill": "deploy", "session": "other", "approve": true, "tools": ["shell"], "window": "another"}).to_string();
    assert_eq!(post(&f.url, &body).0, 200);
    let got = events(&f, 0);
    assert_eq!(got[0].event, WindowEvent::Notification { uuid: "u3".into(), label: "callback".into(), text: "hello".into() });
}

#[test]
fn a_seen_uuid_stays_seen_across_a_restart() {
    let f = fixture();
    assert_eq!(post(&f.url, &json!({"uuid": "u4", "type": "notification", "text": "x"}).to_string()).0, 200);
    let second = Daemon::start(&f.data_path, 0).unwrap();
    let client = Client::find(&f.data_path).unwrap();
    let reg = client.register(&f.root_path).unwrap();
    assert_eq!(second.port(), reg.url.split(':').nth(2).unwrap().split('/').next().unwrap().parse::<u16>().unwrap());
    let (status, reply) = post(&reg.url, &json!({"uuid": "u4", "type": "notification", "text": "x"}).to_string());
    assert_eq!((status, reply["result"].as_str()), (200, Some("duplicate")));
}

#[test]
fn a_journal_callback_writes_only_to_the_journal() {
    let f = fixture();
    let (status, reply) = post(&f.url, &json!({"uuid": "u5", "type": "journal", "text": "the deploy finished", "category": "update", "tags": ["deploy"]}).to_string());
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["detail"]["entry"], 1);
    let got = events(&f, 0);
    let WindowEvent::Journal { record } = &got[0].event else { panic!("{got:?}") };
    assert_eq!((record.body.as_str(), record.category.as_str()), ("the deploy finished", "update"));
    assert_eq!(post(&f.url, &json!({"uuid": "u6", "type": "journal"}).to_string()).0, 400, "no text");
    // Fixed, the same uuid can be sent again: a failure does not burn it.
    assert_eq!(post(&f.url, &json!({"uuid": "u6", "type": "journal", "text": "the second try has its text"}).to_string()).0, 200);
}

#[test]
fn a_flow_update_moves_an_instance_and_never_the_chart() {
    let f = fixture();
    let key = key_for(None, Path::new(&f.root_path));
    let memory = JournalMemory::open(&f.data_path.join("journals"), &key, "t").unwrap();
    let run = |fut: harness_api::BoxFuture<'_, _>| -> harness_api::MapData {
        let mut fut = fut;
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match fut.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(Ok(v)) => v,
            other => panic!("{:?}", other.is_ready()),
        }
    };
    use harness_api::{Memory, NewMapNode};
    let chart = run(memory.map_new(MapKind::Flowchart, "deploy", Some("down")));
    let chart = run(memory.map_add(chart.id, NewMapNode { text: "build".into(), kind: "process".into(), after: Some(chart.root), ..Default::default() }));
    let (root, build) = (chart.root, chart.nodes[1].id);

    let update = |uuid: &str, instance: &str, node: u64| json!({"uuid": uuid, "type": "flowstate_update", "map": chart.id, "instance": instance, "node": node}).to_string();
    assert_eq!(post(&f.url, &update("f1", "req-1", root)).0, 200);
    assert_eq!(post(&f.url, &update("f2", "req-1", build)).0, 200);
    assert_eq!(post(&f.url, &update("f3", "req-2", root)).0, 200);
    let (status, reply) = post(&f.url, &update("f4", "req-1", 999));
    assert_eq!((status, &reply["detail"]["off_chart"]), (200, &json!(true)));

    let got = events(&f, 0);
    let flows: Vec<_> = got.iter().map(|e| match &e.event {
        WindowEvent::Flow { instance, node, off_chart, path, map, .. } => {
            assert_eq!(map.nodes.len(), 2, "the chart itself never changes");
            (instance.clone(), *node, *off_chart, path.clone())
        }
        other => panic!("{other:?}"),
    }).collect();
    assert_eq!(flows[1], ("req-1".to_string(), build, false, vec![root, build]));
    assert_eq!(flows[2], ("req-2".to_string(), root, false, vec![root]), "instances keep their own paths");
    assert_eq!(flows[3], ("req-1".to_string(), 999, true, vec![root, build]), "off-chart is recorded, not added");
    assert_eq!(post(&f.url, &json!({"uuid": "f5", "type": "flowstate_update", "map": 99, "instance": "i", "node": 1}).to_string()).0, 400);
}

#[test]
fn a_poll_waits_for_the_next_event() {
    let f = fixture();
    let (client, window, url) = (f.client.clone(), f.window.clone(), f.url.clone());
    let waiter = std::thread::spawn(move || client.poll(&window, 0).unwrap());
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(post(&url, &json!({"uuid": "p1", "type": "notification", "text": "now"}).to_string()).0, 200);
    let got = waiter.join().unwrap();
    assert_eq!(got.len(), 1);
}

fn with_conditions(toml: &str) -> Fixture {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir_all(root.path().join(".harness")).unwrap();
    std::fs::write(root.path().join(".harness/conditions.toml"), toml).unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let reg = client.register(&root_path).unwrap();
    let data_path = data.path().to_path_buf();
    Fixture { _data: data, _root: root, daemon, client, window: reg.window, url: reg.url, data_path, root_path }
}

fn signal(f: &Fixture, uuid: &str, extra: Value) -> (u16, Value) {
    let mut body = json!({"uuid": uuid, "type": "iteration_failed"});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    post(&f.url, &body.to_string())
}

const THREE: &str = r#"
[[condition]]
name = "pipeline keeps failing"
on = "iteration_failed"
at_least = 3
cooldown_s = 1
policies = [".harness/policies/pipeline.md"]
"#;

#[test]
fn signals_gather_silently_until_the_condition_is_true() {
    let f = with_conditions(THREE);
    for i in 0..2 {
        let (status, reply) = signal(&f, &format!("s{i}"), json!({"iteration": i}));
        assert_eq!((status, reply["detail"]["signal"].as_bool()), (200, Some(true)));
        assert_eq!(reply["detail"]["fired"], json!([]));
    }
    assert_eq!(signal(&f, "s0", json!({})).1["result"], "duplicate", "a repeat is not counted");
    assert!(f.daemon.port() > 0);
    assert_eq!(signal(&f, "s2", json!({"iteration": 2})).1["detail"]["fired"], json!(["pipeline keeps failing"]));
    let got = events(&f, 0);
    assert_eq!(got.len(), 1);
    let WindowEvent::Notification { label, text, .. } = &got[0].event else { panic!("{got:?}") };
    assert_eq!(label, "pipeline keeps failing");
    for part in ["3 signal(s)", "one message, not a stream", "triggers.jsonl", ".harness/policies/pipeline.md", "\"iteration\":2"] {
        assert!(text.contains(part), "{part} in {text}");
    }
}

/// INV-14: a burst of different events is one message; nothing is lost from the log.
#[test]
fn a_burst_is_one_message_and_every_signal_is_logged() {
    let f = with_conditions(THREE);
    for i in 0..100 {
        assert_eq!(signal(&f, &format!("b{i}"), json!({})).0, 200);
    }
    assert_eq!(events(&f, 0).len(), 1);
    assert_eq!(log(&f).iter().filter(|l| l["type"] == "iteration_failed").count(), 100);

    // After the quiet time a new batch can fire, and mentions what was held.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for i in 0..3 {
        signal(&f, &format!("c{i}"), json!({}));
    }
    let got = events(&f, 0);
    assert_eq!(got.len(), 2);
    let WindowEvent::Notification { text, .. } = &got[1].event else { panic!() };
    assert!(text.contains("97 more came during the quiet time"), "{text}");
}

#[test]
fn a_field_rule_counts_only_what_passes_and_a_window_forgets_old_signals() {
    let f = with_conditions(r#"
[[condition]]
name = "slow"
on = "iteration_failed"
field = "ms"
op = "gt"
value = 1000
"#);
    assert_eq!(signal(&f, "a", json!({"ms": 500})).1["detail"]["fired"], json!([]));
    assert_eq!(signal(&f, "w", json!({"ms": "not a number"})).1["detail"]["fired"], json!([]));
    assert_eq!(signal(&f, "b", json!({"ms": 2000})).1["detail"]["fired"], json!(["slow"]));

    let f = with_conditions(r#"
[[condition]]
name = "two quickly"
on = "iteration_failed"
at_least = 2
within_s = 1
"#);
    signal(&f, "x", json!({}));
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert_eq!(signal(&f, "y", json!({})).1["detail"]["fired"], json!([]), "the first one is too old");
    assert_eq!(signal(&f, "z", json!({})).1["detail"]["fired"], json!(["two quickly"]));
}

#[test]
fn other_types_stay_refused_and_a_broken_file_costs_only_the_conditions() {
    let f = with_conditions(THREE);
    assert_eq!(post(&f.url, &json!({"uuid": "o", "type": "something_else"}).to_string()).0, 400);
    let f = with_conditions("this is not toml [[");
    assert_eq!(post(&f.url, &json!({"uuid": "n", "type": "notification", "text": "still works"}).to_string()).0, 200);
    assert_eq!(signal(&f, "q", json!({})).0, 400);
}

/// ADR 0015: the project's callback settings are obeyed, and a reload applies them.
#[test]
fn switching_off_refuses_and_a_reload_applies() {
    let f = with_conditions(THREE);
    assert_eq!(post(&f.url, &json!({"uuid": "k1", "type": "journal", "text": "works"}).to_string()).0, 200);
    assert_eq!(signal(&f, "k2", json!({})).0, 200);

    let mut settings = conditions::ProjectCallbacks::default();
    settings.handlers.insert("journal".into(), "off".into());
    settings.disabled.push("pipeline keeps failing".into());
    conditions::write_project(&f.root_path, &settings).unwrap();
    assert_eq!(conditions::read_project(&f.root_path).unwrap(), settings);

    // Until the reload nothing changes; after it, both are refused.
    assert_eq!(post(&f.url, &json!({"uuid": "k3", "type": "journal", "text": "still"}).to_string()).0, 200);
    let reply = f.client.reload().unwrap();
    assert_eq!(reply["reloaded"], 1);
    let (status, reply) = post(&f.url, &json!({"uuid": "k4", "type": "journal", "text": "no"}).to_string());
    assert_eq!(status, 400);
    assert!(reply["error"].as_str().unwrap().contains("switched off"));
    assert_eq!(signal(&f, "k5", json!({})).0, 400, "a condition switched off gets no signals");
    assert_eq!(post(&f.url, &json!({"uuid": "k6", "type": "notification", "text": "others go on"}).to_string()).0, 200);

    // Back on: the same uuid was not burnt.
    conditions::write_project(&f.root_path, &conditions::ProjectCallbacks::default()).unwrap();
    f.client.reload().unwrap();
    assert_eq!(post(&f.url, &json!({"uuid": "k4", "type": "journal", "text": "yes"}).to_string()).0, 200);
    let base = format!("http://127.0.0.1:{}", f.daemon.port());
    assert_eq!(post(&format!("{base}/reload"), "{}").0, 401);
}

/// A custom handler for the tests: it counts its calls and can pass a message on, or fail.
struct TestHandler {
    calls: std::sync::atomic::AtomicUsize,
}

impl harness_api::CallbackHandler for TestHandler {
    fn name(&self) -> &str {
        "acme-handler"
    }

    fn types(&self) -> Vec<String> {
        vec!["deploy_done".into()]
    }

    fn handle<'a>(&'a self, callback: &'a harness_api::Callback) -> harness_api::BoxFuture<'a, harness_api::PluginResult<harness_api::HandlerEffect>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let outcome = if callback.fields["fail"].as_bool() == Some(true) {
            Err(harness_api::PluginError::new("it broke"))
        } else {
            Ok(harness_api::HandlerEffect::Agent { label: "deploy".into(), text: format!("got {}", callback.fields["env"]) })
        };
        Box::pin(std::future::ready(outcome))
    }
}

fn with_handler() -> (Fixture, std::sync::Arc<TestHandler>) {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let handler = std::sync::Arc::new(TestHandler { calls: Default::default() });
    let daemon = Daemon::start_with(data.path(), 0, vec![handler.clone()]).unwrap();
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let reg = client.register(&root_path).unwrap();
    let data_path = data.path().to_path_buf();
    (Fixture { _data: data, _root: root, daemon, client, window: reg.window, url: reg.url, data_path, root_path }, handler)
}

fn calls(h: &TestHandler) -> usize {
    h.calls.load(std::sync::atomic::Ordering::SeqCst)
}

#[test]
fn a_custom_handler_serves_its_type_once_and_can_tell_the_agent() {
    let (f, h) = with_handler();
    assert_eq!(f.client.handlers().unwrap(), vec![("acme-handler".to_string(), vec!["deploy_done".to_string()])]);
    let body = json!({"uuid": "h1", "type": "deploy_done", "env": "prod"}).to_string();
    let (status, reply) = post(&f.url, &body);
    assert_eq!((status, reply["handler"].as_str()), (200, Some("acme-handler")));
    assert_eq!(post(&f.url, &body).1["result"], "duplicate");
    assert_eq!(calls(&h), 1, "INV-14: a repeat never reaches it");
    assert_eq!(events(&f, 0)[0].event, WindowEvent::Notification { uuid: "h1".into(), label: "deploy".into(), text: "got \"prod\"".into() });
    // A failure answers 400 and does not burn the uuid.
    let failing = json!({"uuid": "h2", "type": "deploy_done", "fail": true}).to_string();
    assert_eq!(post(&f.url, &failing).0, 400);
    assert_eq!(post(&f.url, &json!({"uuid": "h2", "type": "deploy_done"}).to_string()).0, 200);
}

#[test]
fn a_custom_handler_can_be_switched_off_or_chosen_over_a_standard_type() {
    let (f, h) = with_handler();
    let set = |handlers: &[(&str, &str)], disabled: &[&str]| {
        let settings = conditions::ProjectCallbacks {
            handlers: handlers.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            disabled: disabled.iter().map(|d| d.to_string()).collect(),
        };
        conditions::write_project(&f.root_path, &settings).unwrap();
        f.client.reload().unwrap();
    };
    set(&[], &["acme-handler"]);
    assert_eq!(post(&f.url, &json!({"uuid": "o1", "type": "deploy_done"}).to_string()).0, 400);
    assert_eq!(calls(&h), 0);

    set(&[("notification", "acme-handler")], &[]);
    let (_, reply) = post(&f.url, &json!({"uuid": "o2", "type": "notification", "env": "x"}).to_string());
    assert_eq!(reply["handler"], "acme-handler");
    assert_eq!(calls(&h), 1);

    set(&[("notification", "built-in")], &[]);
    let (_, reply) = post(&f.url, &json!({"uuid": "o3", "type": "notification", "text": "plain"}).to_string());
    assert_eq!(reply["handler"], "notification");
    assert_eq!(calls(&h), 1);
    let base = format!("http://127.0.0.1:{}", f.daemon.port());
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    assert_eq!(agent.get(format!("{base}/handlers")).call().unwrap().status().as_u16(), 401);
}

/// A restart keeps the windows, so a callback URL given out earlier still works.
#[test]
fn windows_survive_a_restart() {
    let f = fixture();
    assert_eq!(last_port(&f.data_path), Some(f.daemon.port()));
    let second = Daemon::start(&f.data_path, 0).unwrap();
    let client = Client::find(&f.data_path).unwrap();
    let url = f.url.replace(&f.daemon.port().to_string(), &second.port().to_string());
    assert_eq!(post(&url, &json!({"uuid": "w1", "type": "notification", "text": "still here"}).to_string()).0, 200);
    let got = client.poll(&f.window, 0).unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].seq > 1_000_000, "numbered from the clock");
}

/// journal#822: long polls never queue other requests. Twenty windows' tabs each hold a poll open,
/// more than the fixed pool of 8 it once had; the health check and a registration still answer at once.
#[test]
fn long_polls_do_not_hold_up_other_requests() {
    let f = fixture();
    let mut polls = Vec::new();
    for _ in 0..20 {
        let (client, window) = (f.client.clone(), f.window.clone());
        // Past every event there is: each poll waits for POLL.
        polls.push(std::thread::spawn(move || client.poll(&window, u64::MAX - 1)));
    }
    // Let them all reach the daemon and wait there.
    std::thread::sleep(std::time::Duration::from_millis(500));
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let status = agent.get(format!("http://127.0.0.1:{}/health", f.client.port())).call().unwrap().status().as_u16();
        assert_eq!(status, 200);
        assert!(start.elapsed() < std::time::Duration::from_millis(100), "health took {:?} behind long polls", start.elapsed());
    }
    let start = std::time::Instant::now();
    f.client.register(&f.root_path).unwrap();
    assert!(start.elapsed() < std::time::Duration::from_millis(100), "register took {:?} behind long polls", start.elapsed());
    // A shutdown still closes the port with the polls open.
    f.client.shutdown().unwrap();
    drop(polls);
}
