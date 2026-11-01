//! The real plugins end to end, against a fake provider endpoint on localhost: real HTTP and SSE
//! through `Net`, a real file edit and a real command through the tools, a real session log.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_api::event as event_kinds;
use harness_api::*;
use harness_session::*;
use serde_json::json;

/// Serves one canned SSE body per request and records the requests.
fn fake_endpoint(bodies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for (i, stream) in listener.incoming().enumerate() {
            let mut stream = stream.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let length = text[..head_end].lines().find_map(|l| l.to_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap())).unwrap_or(0);
                    if request.len() >= head_end + 4 + length {
                        break;
                    }
                }
            }
            log.lock().unwrap().push(String::from_utf8_lossy(&request).into_owned());
            let body = bodies.get(i).cloned().unwrap_or_default();
            let response = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    (url, seen)
}

fn sse(events: &[serde_json::Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("x"))).collect()
}

fn anthropic_turn(tool: Option<(&str, &str, serde_json::Value)>, text: &str) -> String {
    let mut events = vec![json!({"type":"message_start","message":{"usage":{"input_tokens":1000,"output_tokens":1}}})];
    events.push(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}));
    events.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-1"}}));
    events.push(json!({"type":"content_block_stop","index":0}));
    events.push(json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}));
    events.push(json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":text}}));
    events.push(json!({"type":"content_block_stop","index":1}));
    let stop = if let Some((id, name, input)) = tool {
        events.push(json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}));
        events.push(json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":input.to_string()}}));
        events.push(json!({"type":"content_block_stop","index":2}));
        "tool_use"
    } else {
        "end_turn"
    };
    events.push(json!({"type":"message_delta","delta":{"stop_reason":stop},"usage":{"output_tokens":100}}));
    events.push(json!({"type":"message_stop"}));
    sse(&events)
}

fn temp_dir(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("harness-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// Runs prompts, answering every permission prompt with `answer`, until each run ends.
fn drive(sessions: &mut Sessions, prompts: &[&str], answer: Answer) -> (Vec<LogEntry>, Vec<RunEndOf>, u32) {
    let (mut entries, mut ends, mut asked) = (Vec::new(), Vec::new(), 0);
    for prompt in prompts {
        sessions.prompt(1, prompt.to_string());
        loop {
            match sessions.next(Duration::from_secs(20)).expect("the session answers") {
                FromSession::Entry { entry, .. } => entries.push(entry),
                FromSession::Ask { tab, .. } => {
                    asked += 1;
                    sessions.answer(tab, answer);
                }
                FromSession::Ended { end, .. } => {
                    ends.push(format!("{end:?}"));
                    break;
                }
                FromSession::Unavailable { message, .. } => panic!("{message}"),
            }
        }
    }
    (entries, ends, asked)
}

type RunEndOf = String;

fn settings(url: &str, data_key: &str) -> Settings {
    let mut s = Settings::default();
    s.providers.insert("anthropic".into(), json!({"base_url": url, "fallbacks": true}));
    s.auth.insert("anthropic".into(), json!({"env": "HARNESS_E2E_NO_SUCH_VAR", "key": data_key}));
    s
}

/// A real coding step: the model edits a file, runs a command that sees the change, and answers.
#[test]
fn a_coding_turn_runs_end_to_end_on_the_real_plugins() {
    let root = temp_dir("root");
    let data = temp_dir("data");
    std::fs::write(format!("{root}/a.txt"), "hello world\n").unwrap();
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("t1", "read", json!({"path": "a.txt"}))), "Reading."),
        anthropic_turn(Some(("t2", "edit", json!({"path": "a.txt", "old_text": "world", "new_text": "harness"}))), "Editing."),
        anthropic_turn(Some(("t3", "shell", json!({"command": "cat a.txt"}))), "Checking."),
        anthropic_turn(None, "It says hello harness."),
    ]);
    let canary = "sk-canary-0123456789";
    let mut sessions = Sessions::new(real_factory(settings(&url, canary), root.clone(), data.clone().into()), root.clone());
    let (entries, ends, asked) = drive(&mut sessions, &["change it"], Answer::AllowOnce);

    assert_eq!(ends, vec!["AwaitingInput"]);
    assert_eq!(std::fs::read_to_string(format!("{root}/a.txt")).unwrap(), "hello harness\n", "the edit happened");
    assert_eq!(asked, 2, "edit and shell ask, the read inside the project does not");
    let results: Vec<(String, bool)> = entries.iter().filter_map(|e| match &e.event {
        Event::ToolResult { output, is_error, .. } => Some((output.clone(), *is_error)),
        _ => None,
    }).collect();
    assert_eq!(results[0], ("hello world\n".to_string(), false));
    assert_eq!(results[2], ("hello harness\n".to_string(), false));

    // What went over the wire.
    let requests = seen.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let first = &requests[0];
    assert!(first.starts_with("POST /v1/messages"), "{first}");
    let head = first.to_lowercase();
    assert!(head.contains("anthropic-version: 2026-10-21"));
    assert!(head.contains(&format!("x-api-key: {canary}")));
    assert!(head.contains("anthropic-beta: server-side-fallback-2026-07-01"));
    let body: serde_json::Value = serde_json::from_str(first.split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["model"], "claude-opus-5-5");
    assert_eq!(body["stream"], true);
    assert_eq!(body["fallbacks"], "default");
    assert_eq!(body["tools"][0]["eager_input_streaming"], true);

    // History replays: thinking blocks unchanged and in place, results as tool_result.
    let last: serde_json::Value = serde_json::from_str(requests[3].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let assistant = &last["messages"][1]["content"];
    assert_eq!(assistant[0], json!({"type":"thinking","thinking":"","signature":"sig-1"}));
    assert_eq!(assistant[1]["type"], "text");
    assert_eq!(assistant[2]["type"], "tool_use");
    assert_eq!(last["messages"][2]["content"][0]["tool_use_id"], "t1");

    // INV-5, INV-6: the log on disk is complete, replays, and holds no key.
    let slug: String = root.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let dir = format!("{data}/sessions/{}", slug.trim_matches('-'));
    let file = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
    let text = std::fs::read_to_string(file).unwrap();
    assert!(!text.contains(canary), "INV-6: the key is not in the log");
    assert_eq!(text.lines().count(), entries.len());
    let stored: Vec<LogEntry> = text.lines().map(|l| harness_api::event::from_line(l).unwrap()).collect();
    assert_eq!(stored, entries);
    let state = SessionState::fold(stored.iter().map(|e| &e.event));
    assert_eq!(state.turns, 4);
    assert_eq!(state.usage.output, 400);
    assert_eq!(state.cost_micro_usd, 4 * (1000 * 4 + 100 * 20), "priced by the table");
}

#[test]
fn a_denied_write_does_not_happen_and_the_model_hears_it() {
    let root = temp_dir("deny");
    let data = temp_dir("deny-data");
    std::fs::write(format!("{root}/a.txt"), "keep\n").unwrap();
    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("t1", "edit", json!({"path": "a.txt", "old_text": "keep", "new_text": "gone"}))), "Editing."),
        anthropic_turn(None, "Fine."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.into()), root.clone());
    let (entries, _, asked) = drive(&mut sessions, &["try"], Answer::Deny);
    assert_eq!(asked, 1);
    assert_eq!(std::fs::read_to_string(format!("{root}/a.txt")).unwrap(), "keep\n");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::ToolResult { output, is_error: true, .. } if output.contains("denied"))));
}

#[test]
fn an_openai_compatible_endpoint_runs_a_turn_with_a_tool() {
    let root = temp_dir("oai");
    let data = temp_dir("oai-data");
    let chunk = |v: serde_json::Value| format!("data: {v}\n\n");
    let first = [
        chunk(json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read","arguments":"{\"path\":\"b.txt\"}"}}]},"finish_reason":"tool_calls"}]})),
        chunk(json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":2}})),
        "data: [DONE]\n\n".to_string(),
    ].concat();
    let second = [chunk(json!({"choices":[{"delta":{"content":"done"},"finish_reason":"stop"}]})), "data: [DONE]\n\n".to_string()].concat();
    std::fs::write(format!("{root}/b.txt"), "local\n").unwrap();
    let (url, seen) = fake_endpoint(vec![first, second]);
    let mut s = Settings { provider: "local".into(), model: "qwen".into(), ..Default::default() };
    s.providers.insert("local".into(), json!({"kind": "openai", "base_url": format!("{url}/v1")}));
    s.auth.insert("local".into(), json!({"none": true}));
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.into()), root);
    let (entries, ends, _) = drive(&mut sessions, &["go"], Answer::Deny);
    assert_eq!(ends, vec!["AwaitingInput"]);
    assert!(entries.iter().any(|e| matches!(&e.event, Event::ToolResult { output, .. } if output == "local\n")));
    let requests = seen.lock().unwrap();
    assert!(requests[0].starts_with("POST /v1/chat/completions"));
    assert!(!requests[0].to_lowercase().contains("authorization:"), "no key, no header");
    assert!(requests[1].contains("\"role\":\"tool\""));
}

#[test]
fn a_missing_key_is_a_clear_message_and_the_session_goes_on_failing_softly() {
    let root = temp_dir("nokey");
    let data = temp_dir("nokey-data");
    let mut s = Settings::default();
    s.auth.insert("anthropic".into(), json!({"env": "HARNESS_E2E_NO_SUCH_VAR"}));
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.into()), root);
    let (entries, ends, _) = drive(&mut sessions, &["hi"], Answer::Deny);
    assert!(ends[0].contains("no API key for anthropic"), "{ends:?}");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::Error { message, .. } if message.contains("HARNESS_E2E_NO_SUCH_VAR"))));
}

/// INV-3: every built-in tool refuses what it was not granted, even if the loop were to call it.
#[test]
fn no_tool_acts_outside_what_was_granted() {
    use harness_tools_builtin::{Edit, Fetch, Read, Search, Shell};
    let root = temp_dir("escape");
    let marker = format!("{root}/marker");
    std::fs::write(format!("{root}/a.txt"), "secret\n").unwrap();
    let cx = |granted: Vec<Capability>| ToolContext { platform: harness_platform_native::platform(std::path::PathBuf::from(&root)), granted, root: root.clone() };
    let block = |f: std::pin::Pin<Box<dyn std::future::Future<Output = ToolOutput> + '_>>| pollster::block_on(f);
    let cases: Vec<(Box<dyn Tool>, serde_json::Value)> = vec![
        (Box::new(Read), json!({"path": "a.txt"})),
        (Box::new(Edit), json!({"path": "a.txt", "old_text": "", "new_text": "x"})),
        (Box::new(Shell), json!({"command": format!("touch {marker}")})),
        (Box::new(Search), json!({"text": "secret"})),
        (Box::new(Fetch), json!({"url": "http://127.0.0.1:9/x"})),
    ];
    for (tool, input) in &cases {
        let none = cx(vec![]);
        let out = block(tool.execute(input, &none));
        assert!(out.is_error && out.text.contains("not granted"), "{}: {}", tool.spec().name, out.text);
        // A different grant than the call needs does not count either.
        let other = cx(vec![Capability::ReadPath("other".into()), Capability::Command("ls".into())]);
        let out = block(tool.execute(input, &other));
        assert!(out.is_error && out.text.contains("not granted"), "{}: {}", tool.spec().name, out.text);
    }
    assert_eq!(std::fs::read_to_string(format!("{root}/a.txt")).unwrap(), "secret\n");
    assert!(!std::path::Path::new(&marker).exists());
    // The declared capability is what the policy saw: granting it makes the call work.
    let read = Read;
    let needs = read.capabilities(&json!({"path": "a.txt"})).unwrap();
    let granted = cx(needs);
    assert_eq!(block(read.execute(&json!({"path": "a.txt"}), &granted)).text, "secret\n");
}

/// INV-5: a session continues from its log, across a restart of everything.
#[test]
fn a_session_resumes_from_its_log() {
    let root = temp_dir("resume");
    let data = temp_dir("resume-data");
    let (url, seen) = fake_endpoint(vec![anthropic_turn(None, "First answer."), anthropic_turn(None, "Second answer.")]);
    let factory = real_factory(settings(&url, "k"), root.clone(), data.clone().into());

    let mut first = Sessions::new(factory.clone(), root.clone());
    let (before, _, _) = drive(&mut first, &["first question\nwith a second line"], Answer::Deny);
    drop(first);

    let listed = recent(&data, &root);
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].title.as_str(), listed[0].model.as_str()), ("first question", "claude-opus-5-5"));
    assert_eq!(read_log(&data, &root, &listed[0].id), before);

    let mut second = Sessions::new(factory, root.clone());
    second.resume(1, &listed[0].id);
    let (after, ends, _) = drive(&mut second, &["second question"], Answer::Deny);
    assert_eq!(ends, vec!["AwaitingInput"]);
    assert_eq!(after[0].seq, before.last().unwrap().seq + 1, "the numbering goes on");

    let body: serde_json::Value = serde_json::from_str(seen.lock().unwrap()[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "the old history went along");
    assert_eq!(messages[0]["content"][0]["text"], "first question\nwith a second line");

    let all = read_log(&data, &root, &listed[0].id);
    assert_eq!(all.len(), before.len() + after.len());
    assert!(all.iter().enumerate().all(|(i, e)| e.seq == i as u64), "no gap in the log");
    assert_eq!(recent(&data, &root).len(), 1, "still one session");
}

/// A log cut off mid-tool still gives a history the API accepts, and a torn last line is dropped.
#[test]
fn a_log_cut_off_mid_tool_resumes_with_an_interrupted_result() {
    let root = temp_dir("cut");
    let data = temp_dir("cut-data");
    let (url, seen) = fake_endpoint(vec![anthropic_turn(None, "Picking up.")]);
    let factory = real_factory(settings(&url, "k"), root.clone(), data.clone().into());
    let call = ToolCall { id: "t9".into(), name: "shell".into(), input: json!({"command": "sleep 100"}) };
    let log = [
        LogEntry { seq: 0, at_ms: 1, event: Event::UserMessage { text: "go".into() } },
        LogEntry { seq: 1, at_ms: 2, event: Event::TaskState { to: TaskState::Running, reason: None } },
        LogEntry { seq: 2, at_ms: 3, event: Event::ModelToolCall { call: call.clone() } },
        LogEntry { seq: 3, at_ms: 4, event: Event::ToolRequested { call, capabilities: vec![] } },
    ];
    let slug: String = root.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    let dir = format!("{data}/sessions/{}", slug.trim_matches('-'));
    std::fs::create_dir_all(&dir).unwrap();
    let mut text: String = log.iter().map(|e| harness_api::event::to_line(e) + "\n").collect();
    text.push_str("{\"seq\":4,\"at_ms");
    std::fs::write(format!("{dir}/old.jsonl"), text).unwrap();
    assert_eq!(read_log(&data, &root, "old").len(), 4);

    let mut sessions = Sessions::new(factory, root);
    sessions.resume(1, "old");
    let (entries, ends, _) = drive(&mut sessions, &["go on"], Answer::Deny);
    assert_eq!(ends, vec!["AwaitingInput"]);
    assert!(matches!(&entries[0].event, Event::ToolResult { id, is_error: true, .. } if id == "t9"));
    let body: serde_json::Value = serde_json::from_str(seen.lock().unwrap()[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], "t9");
}

/// IF-6: shell hooks add context, veto and fail, each as specified.
#[test]
fn shell_hooks_add_context_veto_and_fail() {
    let hook = |event: &str, command: &str| HookSetting { event: Some(event.into()), command: command.into(), name: Some(format!("{event}.sh")) };
    let root = temp_dir("hooks");
    let data = temp_dir("hooks-data");
    std::fs::write(format!("{root}/a.txt"), "x\n").unwrap();
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("t1", "read", json!({"path": "a.txt"}))), "Reading."),
        anthropic_turn(Some(("t2", "shell", json!({"command": "echo ran > ran.txt"}))), "Running."),
        anthropic_turn(None, "Done."),
    ]);
    let mut s = settings(&url, "k");
    s.hooks = vec![
        hook("SessionStart", "echo Journal your decisions."),
        // Vetoes the shell tool only: the hook sees the call.
        hook("PreToolUse", if cfg!(windows) {
            r#"echo %HOOK_CALL% | findstr /C:"shell" >nul && (echo no shell today 1>&2 & exit 2)"#
        } else {
            r#"case "$HOOK_CALL" in *'"name":"shell"'*) echo no shell today >&2; exit 2;; esac"#
        }),
        hook("PostToolUse", "exit 7"),
    ];
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.into()), root.clone());
    let (entries, ends, asked) = drive(&mut sessions, &["go"], Answer::AllowOnce);
    assert_eq!(ends, vec!["AwaitingInput"]);
    assert_eq!(asked, 0, "the veto came before the permission prompt");
    assert!(!std::path::Path::new(&format!("{root}/ran.txt")).exists(), "the vetoed command did not run");

    let requests = seen.lock().unwrap();
    let first: serde_json::Value = serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    assert!(first["system"].as_str().unwrap().contains("Journal your decisions."));
    let third: serde_json::Value = serde_json::from_str(requests[2].split("\r\n\r\n").nth(1).unwrap()).unwrap();
    let results = third["messages"].to_string();
    assert!(results.contains("no shell today"), "the model hears why");

    let outcomes: Vec<&event_kinds::HookOutcome> = entries.iter().filter_map(|e| match &e.event {
        Event::HookRan { outcome, .. } => Some(outcome),
        _ => None,
    }).collect();
    assert!(outcomes.iter().any(|o| matches!(o, event_kinds::HookOutcome::Failed { message } if message.contains("exit 7"))), "{outcomes:?}");
    assert!(outcomes.iter().any(|o| matches!(o, event_kinds::HookOutcome::Blocked { .. })));
    // A hook that does not apply leaves no trace.
    let stop_cards = entries.iter().filter(|e| matches!(&e.event, Event::HookRan { point: event_kinds::HookPointKind::Stop, .. })).count();
    assert_eq!(stop_cards, 0);
    // The failing PostToolUse hook blocked nothing: the read's result still reached the model.
    assert!(third["messages"].to_string().contains("\"x\\n\""));
}

/// IF-7, CON-22: the agent's memory tools reach a real journal and real maps, and every checkout
/// of a repository shares them.
#[test]
fn memory_tools_write_a_real_journal_shared_by_every_checkout() {
    let data = temp_dir("mem-data");
    let git = |root: &str, url: &str| {
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", url]] {
            let ok = std::process::Command::new("git").arg("-C").arg(root).args(args).status().unwrap().success();
            assert!(ok);
        }
    };
    let (a, b) = (temp_dir("mem-a"), temp_dir("mem-b"));
    git(&a, "git@github.com:acme/app.git");
    git(&b, "https://github.com/acme/app");

    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("t1", "journal_add", json!({"body": "Chose SQLite because readers block.", "category": "decision", "tags": ["sqlite"], "flags": ["important"]}))), "Noting."),
        anthropic_turn(Some(("t2", "flowchart_new", json!({"title": "deploy", "direction": "down"}))), "Charting."),
        anthropic_turn(Some(("t3", "flowchart_add", json!({"map": 1, "text": "build", "kind": "process", "after": 1, "label": "go"}))), "Adding."),
        anthropic_turn(None, "Done."),
        anthropic_turn(Some(("t4", "journal_search", json!({"query": "SQLite"}))), "Looking."),
        anthropic_turn(Some(("t5", "flowchart_show", json!({"map": 1}))), "Showing."),
        anthropic_turn(None, "Found it."),
    ]);
    let mut first = Sessions::new(real_factory(settings(&url, "k"), a.clone(), data.clone().into()), a);
    let (entries, _, asked) = drive(&mut first, &["note it"], Answer::Deny);
    assert_eq!(asked, 0, "memory writes need no prompt");
    let outputs = |entries: &[LogEntry]| -> Vec<serde_json::Value> {
        entries.iter().filter_map(|e| match &e.event {
            Event::ToolResult { output, is_error, .. } => {
                assert!(!is_error, "{output}");
                Some(serde_json::from_str(output).unwrap())
            }
            _ => None,
        }).collect()
    };
    let results = outputs(&entries);
    assert_eq!(results[0]["id"], 1);
    assert_eq!(results[0]["flags"][0], "important");
    assert_eq!(results[1]["kind"], "flowchart");
    assert_eq!(results[2]["edges"][0]["label"], "go");

    // Another checkout of the same repository, another process's worth of state.
    let mut second = Sessions::new(real_factory(settings(&url, "k"), b.clone(), data.clone().into()), b);
    let (entries, _, _) = drive(&mut second, &["find it"], Answer::Deny);
    let results = outputs(&entries);
    assert_eq!(results[0]["entries"][0]["body"], "Chose SQLite because readers block.");
    assert_eq!(results[1]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(std::fs::read_dir(format!("{data}/journals")).unwrap().filter(|f| f.as_ref().unwrap().path().extension().is_some_and(|e| e == "db")).count(), 1, "one journal file");
}

/// INV-6 for the debug wire log: bodies and answers are there, keys are not. A subprocess-free
/// check of the wrapper on the real network path.
#[test]
fn the_wire_log_shows_bodies_and_never_keys() {
    let root = temp_dir("wire");
    let data = temp_dir("wire-data");
    let log = format!("{data}/wire.log");
    // SAFETY: this test binary reads this variable nowhere else, and no other test sets it.
    unsafe { std::env::set_var("MULTITOOL_WIRE_LOG", &log) };
    let (url, _) = fake_endpoint(vec![anthropic_turn(None, "Visible answer.")]);
    let canary = "sk-wire-canary-987";
    let mut sessions = Sessions::new(real_factory(settings(&url, canary), root.clone(), data.into()), root);
    drive(&mut sessions, &["visible question"], Answer::Deny);
    unsafe { std::env::remove_var("MULTITOOL_WIRE_LOG") };
    let text = std::fs::read_to_string(log).unwrap();
    assert!(text.contains(">>> POST") && text.contains("visible question"), "{text}");
    assert!(text.contains("<<< 200") && text.contains("Visible answer."), "{text}");
    assert!(!text.contains(canary), "INV-6: the key is not in the wire log");
}

/// ADR 0015: the agent can list what is plugged in and ask for a reload, with permission.
#[test]
fn the_agent_lists_plugins_and_asks_for_a_reload() {
    let root = temp_dir("host");
    let data = temp_dir("host-data");
    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("t1", "plugins_list", json!({}))), "Looking."),
        anthropic_turn(Some(("t2", "plugins_reload", json!({}))), "Reloading."),
        anthropic_turn(None, "Done."),
        anthropic_turn(Some(("t3", "plugins_reload", json!({}))), "Again."),
        anthropic_turn(None, "Fine."),
    ]);
    let reload: std::sync::Arc<std::sync::atomic::AtomicBool> = Default::default();
    let host = Host { plugins: std::sync::Arc::new(|| json!({"provider": "anthropic", "callback_handlers_custom": [{"name": "acme"}]})), reload: reload.clone(), ..Default::default() };
    let mut sessions = Sessions::new(with_host(settings(&url, "k"), root.clone(), data.into(), host), root);
    let (entries, _, asked) = drive(&mut sessions, &["what is plugged in?"], Answer::AllowOnce);
    assert_eq!(asked, 1, "listing runs by itself, reloading asks");
    assert!(reload.swap(false, std::sync::atomic::Ordering::SeqCst), "the frontend is told");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::ToolResult { output, .. } if output.contains("acme"))));

    let (_, _, asked) = drive(&mut sessions, &["again"], Answer::Deny);
    assert_eq!(asked, 1);
    assert!(!reload.load(std::sync::atomic::Ordering::SeqCst), "a denied reload does nothing");
}

/// M3: the context is built from memory, and a long session compacts without losing it.
#[test]
fn context_comes_from_memory_and_survives_compaction() {
    let root = temp_dir("ctx");
    let data = temp_dir("ctx-data");
    let filler = "word ".repeat(400);
    let mut turns = vec![
        anthropic_turn(Some(("s1", "journal_add", json!({"body": "Prices are stored in cents, always.", "category": "specification", "tags": ["money"], "flags": ["specification"]}))), "Noting."),
        anthropic_turn(Some(("s2", "journal_add", json!({"body": "Never call the bank on weekends.", "category": "decision", "tags": ["bank"], "flags": ["constraint"]}))), "Noting."),
        anthropic_turn(Some(("s3", "journal_add", json!({"body": "An old rule that no longer holds.", "category": "decision", "tags": ["old"], "flags": ["constraint", "deprecated"]}))), "Noting."),
        anthropic_turn(Some(("s4", "flowchart_new", json!({"title": "the deploy flow"}))), "Charting."),
        anthropic_turn(None, "Done."),
    ];
    for i in 0..6 {
        turns.push(anthropic_turn(Some((&format!("t{i}"), "read", json!({"path": "big.txt"}))), "Reading."));
        turns.push(anthropic_turn(None, "Read it."));
    }
    let (url, seen) = fake_endpoint(turns);
    std::fs::write(format!("{root}/big.txt"), &filler).unwrap();
    let mut s = settings(&url, "k");
    s.compact_at_tokens = Some(1500);
    s.hooks = vec![HookSetting { event: Some("PostCompact".into()), command: "echo Re-read the specifications.".into(), name: Some("after-compact".into()) }];
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    let mut prompts: Vec<String> = vec!["remember these".to_string()];
    prompts.extend((0..6).map(|i| format!("ask number {i}")));
    let prompts_ref: Vec<&str> = prompts.iter().map(String::as_str).collect();
    let (entries, ends, _) = drive(&mut sessions, &prompts_ref, Answer::Deny);
    assert!(ends.iter().all(|e| e == "AwaitingInput"), "{ends:?}");

    let requests: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|r| serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap()).collect();
    let first_system = requests[0]["system"].as_str().unwrap();
    assert!(!first_system.contains("Prices are stored"), "nothing in memory yet");
    let last = requests.last().unwrap();
    let system = last["system"].as_str().unwrap();
    assert!(system.contains("Prices are stored in cents") && system.contains("Never call the bank"), "{system}");
    assert!(!system.contains("no longer holds"), "a deprecated entry does not bind");
    assert!(system.contains("the deploy flow"), "{system}");

    let cuts: Vec<usize> = entries.iter().filter_map(|e| if let Event::Compacted { upto, .. } = e.event { Some(upto) } else { None }).collect();
    assert!(!cuts.is_empty(), "the budget was passed");
    assert!(cuts.windows(2).all(|w| w[0] < w[1]), "cuts only move forward: {cuts:?}");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::HookRan { point: event_kinds::HookPointKind::PostCompact, .. })));
    assert!(system.contains("Re-read the specifications."), "the hook's context reaches the model");

    let messages = last["messages"].as_array().unwrap();
    let full: usize = entries.iter().filter(|e| matches!(e.event, Event::UserMessage { .. })).count();
    assert_eq!(full, 7);
    assert!(messages.len() < 2 * full + 4, "the request is shorter than the conversation: {}", messages.len());
    assert_eq!(messages[0]["role"], "user");
    assert!(messages[0].to_string().contains("was compacted") && messages[0].to_string().contains("remember these"), "{}", messages[0]);
    for pair in messages.windows(2) {
        assert_ne!(pair[0]["role"], pair[1]["role"], "roles alternate");
    }

    // The log keeps everything, and a resume replays to the same cut.
    let listed = recent(&data, &root);
    let stored = read_log(&data, &root, &listed[0].id);
    let state = SessionState::fold(stored.iter().map(|e| &e.event));
    assert_eq!(state.compacted_upto, *cuts.last().unwrap());
    assert_eq!(stored.iter().filter(|e| matches!(e.event, Event::UserMessage { .. })).count(), 7, "INV-5: nothing is gone");
}

/// CON-20: with a daemon running, a session's memory goes through it.
#[test]
fn a_sessions_memory_goes_through_a_running_daemon() {
    let root = temp_dir("own");
    let data = temp_dir("own-data");
    let _daemon = harness_daemon::Daemon::start(std::path::Path::new(&data), 0).unwrap();
    let client = harness_daemon::client::Client::find(std::path::Path::new(&data)).unwrap();
    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("s1", "journal_add", json!({"body": "Owned by the daemon.", "category": "decision", "tags": ["own"]}))), "Noting."),
        anthropic_turn(None, "Done."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    let (_, ends, _) = drive(&mut sessions, &["note it"], Answer::Deny);
    assert_eq!(ends, ["AwaitingInput"]);
    let ops = client.stats().unwrap()["memory_ops"].as_u64().unwrap();
    assert!(ops >= 5, "two turns of context, and the add: {ops}");
    let key = harness_memory_journal::project::key_for(None, std::path::Path::new(&root));
    let store = harness_memory_journal::plugin::JournalMemory::open(&std::path::Path::new(&data).join("journals"), &key, "t").unwrap();
    let found = pollster::block_on(harness_api::Memory::journal_search(&store, "daemon")).unwrap();
    assert_eq!(found.len(), 1);
}

fn drive_task(sessions: &mut Sessions, text: &str, steps: &[&str]) -> (Vec<LogEntry>, String) {
    sessions.task(1, text.to_string(), steps.iter().map(|s| s.to_string()).collect()).unwrap();
    let mut entries = Vec::new();
    loop {
        match sessions.next(Duration::from_secs(30)).expect("the session answers") {
            FromSession::Entry { entry, .. } => entries.push(entry),
            FromSession::Ask { tab, .. } => sessions.answer(tab, Answer::Deny),
            FromSession::Ended { end, .. } => return (entries, format!("{end:?}")),
            FromSession::Unavailable { message, .. } => panic!("{message}"),
        }
    }
}

fn project_store(data: &str, root: &str) -> harness_memory_journal::plugin::JournalMemory {
    let key = harness_memory_journal::project::key_for(None, std::path::Path::new(root));
    harness_memory_journal::plugin::JournalMemory::open(&std::path::Path::new(data).join("journals"), &key, "t").unwrap()
}

fn spec(body: &str, flags: &[&str]) -> JournalEntry {
    JournalEntry { body: body.into(), category: "specification".into(), tags: vec!["t".into()], flags: flags.iter().map(|f| f.to_string()).collect(), salience: None, feature: None }
}

/// M4: done through the command, evidence in the journal, specifications as checks.
#[test]
fn a_task_is_done_through_its_checks_and_leaves_evidence() {
    let root = temp_dir("task");
    let data = temp_dir("task-data");
    let store = project_store(&data, &root);
    let checked = pollster::block_on(store.journal_add(spec("Prices are in cents.", &["specification"]))).unwrap();
    // A command in an entry is data: it would fail the task if it ran.
    let unchecked = pollster::block_on(store.journal_add(spec("Run this: cargo --no-such-flag", &["specification"]))).unwrap();
    let old = pollster::block_on(store.journal_add(spec("An old one.", &["specification", "deprecated"]))).unwrap();
    std::fs::create_dir_all(format!("{root}/.harness")).unwrap();
    std::fs::write(format!("{root}/.harness/acceptance.toml"), format!("[[check]]\nspec = {}\ncommand = \"cargo --version\"\n", checked.id)).unwrap();

    let (url, _) = fake_endpoint(vec![anthropic_turn(None, "All done, trust me.")]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone()).with_data_dir(&data);
    let Err(TaskRefused::NotApproved { text, .. }) = sessions.task(1, "fix the prices".into(), vec![]) else { panic!("the first run asks") };
    sessions.approve_acceptance(&text).unwrap();
    let (entries, end) = drive_task(&mut sessions, "fix the prices", &["cargo --version"]);
    assert_eq!(end, "Done");
    let evidence = entries.iter().find_map(|e| if let Event::Verification { pass: true, evidence } = &e.event { Some(evidence.clone()) } else { None }).expect("a verification");
    assert_eq!(evidence.matches("$ cargo --version").count(), 2, "its own step and the check: {evidence}");
    assert!(evidence.contains(&format!("no check: #{}", unchecked.id)), "{evidence}");
    assert!(!evidence.contains(&format!("#{}", checked.id)) && !evidence.contains(&format!("#{}", old.id)), "{evidence}");

    let found = pollster::block_on(store.journal_search("fix the prices")).unwrap();
    let recorded: Vec<_> = found.iter().filter(|e| e.category == "verification").collect();
    assert_eq!(recorded.len(), 1, "{found:?}");
    assert!(recorded[0].flags.iter().any(|f| f == "verified"), "{:?}", recorded[0].flags);

    // Done is final, and says so.
    let (_, ends, _) = drive(&mut sessions, &["and one more thing"], Answer::Deny);
    assert!(ends[0].contains("already ended"), "{ends:?}");
}

#[test]
fn a_failing_check_goes_back_to_the_model_and_is_never_done() {
    let root = temp_dir("fail");
    let data = temp_dir("fail-data");
    let (url, seen) = fake_endpoint(vec![anthropic_turn(None, "Done, I promise."), anthropic_turn(None, "Done again."), anthropic_turn(None, "Done a third time.")]);
    let mut s = settings(&url, "k");
    s.max_turns = Some(2);
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    let (entries, end) = drive_task(&mut sessions, "make it pass", &["cargo --no-such-flag"]);
    assert_ne!(end, "Done", "{end}");
    assert!(entries.iter().any(|e| matches!(e.event, Event::Verification { pass: false, .. })));
    assert!(!entries.iter().any(|e| matches!(e.event, Event::Verification { pass: true, .. })));
    assert!(seen.lock().unwrap().iter().any(|r| r.contains("Verification failed")), "the model saw the evidence");
    let found = pollster::block_on(project_store(&data, &root).journal_search("make it pass")).unwrap();
    assert!(!found.is_empty());
    assert!(found.iter().all(|e| !e.flags.iter().any(|f| f == "verified")), "{found:?}");
}

#[test]
fn a_task_with_a_bad_file_or_nothing_to_check_is_refused() {
    let root = temp_dir("refuse");
    let data = temp_dir("refuse-data");
    assert!(protocol_for(Some(&data), &root, vec![]).unwrap_err().to_string().contains("needs something to check"));
    std::fs::create_dir_all(format!("{root}/.harness")).unwrap();
    std::fs::write(format!("{root}/.harness/acceptance.toml"), "[[check]]\nspec = \"not a number\"\n").unwrap();
    assert!(protocol_for(Some(&data), &root, vec!["cargo --version".into()]).unwrap_err().to_string().contains("acceptance.toml"));
}

/// CON-51: a project's commands run only as the user approved them.
#[test]
fn a_projects_checks_run_only_as_approved() {
    let root = temp_dir("approve");
    let data = temp_dir("approve-data");
    let file = format!("{root}/.harness/acceptance.toml");
    let first = "[[check]]\nspec = 1\ncommand = \"first\"\n";

    // With no file, the user's own commands need no approval.
    assert_eq!(protocol_for(Some(&data), &root, vec!["mine".into()]).unwrap().steps, ["mine"]);

    std::fs::create_dir_all(format!("{root}/.harness")).unwrap();
    std::fs::write(&file, first).unwrap();
    let refused = protocol_for(Some(&data), &root, vec!["mine".into()]).unwrap_err();
    assert_eq!(refused, TaskRefused::NotApproved { commands: vec!["first".into()], text: first.into() });
    assert!(protocol_for(None, &root, vec![]).is_err(), "no place for approvals: never approved");

    approve_acceptance(&data, &root, first).unwrap();
    approve_acceptance(&data, &root, first).unwrap();
    let protocol = protocol_for(Some(&data), &root, vec!["mine".into()]).unwrap();
    assert_eq!((protocol.steps, protocol.specs), (vec!["mine".to_string(), "first".to_string()], vec![1]));

    // A change asks again, and approving what was shown does not cover what came after.
    let second = first.replace("first", "second");
    std::fs::write(&file, &second).unwrap();
    assert!(matches!(protocol_for(Some(&data), &root, vec![]), Err(TaskRefused::NotApproved { commands, .. }) if commands == ["second"]));
    std::fs::write(&file, second.replace("second", "third")).unwrap();
    approve_acceptance(&data, &root, &second).unwrap();
    assert!(matches!(protocol_for(Some(&data), &root, vec![]), Err(TaskRefused::NotApproved { commands, .. }) if commands == ["third"]));
}

/// ADR 0016: in plan mode nothing changes whatever is answered, and only the user ends it.
#[test]
fn plan_mode_refuses_changes_and_only_the_user_ends_it() {
    let root = temp_dir("plan");
    let data = temp_dir("plan-data");
    std::fs::write(format!("{root}/a.txt"), "before").unwrap();
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("p1", "plan_enter", json!({}))), "Planning."),
        anthropic_turn(Some(("p2", "edit", json!({"path": "a.txt", "old_text": "before", "new_text": "after"}))), "Editing."),
        anthropic_turn(Some(("p3", "shell", json!({"command": "cargo --version > b.txt"}))), "Shell."),
        anthropic_turn(Some(("p4", "read", json!({"path": "a.txt"}))), "Reading."),
        anthropic_turn(Some(("p5", "journal_add", json!({"body": "Change a.txt.", "category": "decision", "tags": ["plan"]}))), "Noting."),
        anthropic_turn(Some(("p6", "plan_exit", json!({"plan": "Change a.txt."}))), "Asking."),
        anthropic_turn(None, "Waiting for you."),
    ]);
    let mut s = settings(&url, "k");
    s.hooks = vec![HookSetting { event: Some("PlanEnter".into()), command: "echo planning".into(), name: Some("on-plan".into()) }];
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    // Everything is answered yes: the refusals cannot be approved, and plan_exit can.
    let (entries, _, asked) = drive(&mut sessions, &["plan the change"], Answer::AlwaysAllow);
    assert_eq!(std::fs::read_to_string(format!("{root}/a.txt")).unwrap(), "before");
    assert!(!std::path::Path::new(&format!("{root}/b.txt")).exists());
    assert_eq!(asked, 1, "only plan_exit asked: {entries:?}");
    let decision = |id: &str| entries.iter().find_map(|e| if let Event::ToolDecision { id: i, allowed, by, .. } = &e.event { (i == id).then(|| (*allowed, format!("{by:?}"))) } else { None });
    assert_eq!(decision("p2"), Some((false, "Mode".to_string())));
    assert_eq!(decision("p3"), Some((false, "Mode".to_string())));
    assert_eq!(decision("p4"), Some((true, "Policy".to_string())));
    assert_eq!(decision("p6"), Some((true, "User".to_string())));
    let modes: Vec<(bool, String)> = entries.iter().filter_map(|e| if let Event::PlanMode { on, by } = &e.event { Some((*on, by.clone())) } else { None }).collect();
    assert_eq!(modes, [(true, "agent".to_string()), (false, "user".to_string())]);
    assert!(entries.iter().any(|e| matches!(&e.event, Event::HookRan { point: event_kinds::HookPointKind::PlanEnter, .. })));

    let requests: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|r| serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap()).collect();
    assert!(!requests[0]["system"].as_str().unwrap().contains("PLAN MODE"));
    assert!(requests[1]["system"].as_str().unwrap().contains("PLAN MODE"));
    assert!(!requests.last().unwrap()["system"].as_str().unwrap().contains("PLAN MODE"), "gone once off");
    assert!(requests[0]["tools"].to_string().contains("plan_exit"));
}

#[test]
fn a_refused_plan_stays_on_and_a_resume_restores_the_mode() {
    let root = temp_dir("plan2");
    let data = temp_dir("plan2-data");
    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("q1", "plan_exit", json!({"plan": "too early"}))), "Leaving."),
        anthropic_turn(Some(("q2", "plan_enter", json!({}))), "Entering."),
        anthropic_turn(Some(("q3", "plan_exit", json!({"plan": "my plan"}))), "Asking."),
        anthropic_turn(Some(("q4", "plan_exit", json!({}))), "Empty."),
        anthropic_turn(None, "Asking what to change."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    let (entries, _, asked) = drive(&mut sessions, &["plan it"], Answer::Deny);
    assert_eq!(asked, 1);
    let result = |id: &str| entries.iter().find_map(|e| if let Event::ToolResult { id: i, output, is_error } = &e.event { (i == id).then(|| (output.clone(), *is_error)) } else { None }).unwrap();
    assert!(result("q1").1 && result("q1").0.contains("not on"), "{:?}", result("q1"));
    assert!(result("q3").1 && result("q3").0.contains("did not approve"), "{:?}", result("q3"));
    assert!(result("q4").1 && result("q4").0.contains("required"), "{:?}", result("q4"));

    let listed = recent(&data, &root);
    let stored = read_log(&data, &root, &listed[0].id);
    assert!(SessionState::fold(stored.iter().map(|e| &e.event)).plan_mode, "still on, and a replay says so");
}

#[test]
fn the_user_switches_plan_mode_and_a_task_does_not_finish_in_it() {
    let root = temp_dir("plan3");
    let data = temp_dir("plan3-data");
    let (url, _) = fake_endpoint(vec![anthropic_turn(None, "Done, I say."), anthropic_turn(None, "Now done.")]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    sessions.plan(1, true);
    let (entries, end) = drive_task(&mut sessions, "do it", &["cargo --version"]);
    assert_eq!(end, "AwaitingInput", "no verification in plan mode");
    assert!(!entries.iter().any(|e| matches!(e.event, Event::Verification { .. })));
    assert!(entries.iter().any(|e| matches!(&e.event, Event::PlanMode { on: true, by } if by == "user")));
    sessions.plan(1, false);
    let (entries, end) = drive_task(&mut sessions, "do it now", &["cargo --version"]);
    assert_eq!(end, "Done");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::PlanMode { on: false, .. })));
}

/// ADR 0017: in an initialized project the agent is told the file, reminded once, and never stopped.
#[test]
fn the_docs_mirror_prompts_for_one_turn_and_blocks_nothing() {
    let root = temp_dir("mirror");
    let data = temp_dir("mirror-data");
    pollster::block_on(harness_docs_mirror::project_init(&harness_platform_native::NativeFs, &root)).unwrap();
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("m1", "journal_add", json!({"body": "Never on weekends.", "category": "decision", "tags": ["x"], "flags": ["constraint"]}))), "Noting."),
        anthropic_turn(None, "Noted."),
        anthropic_turn(None, "On the next message."),
        anthropic_turn(None, "And the one after."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    let (entries, ends, _) = drive(&mut sessions, &["note it", "next", "after"], Answer::Deny);
    assert!(ends.iter().all(|e| e == "AwaitingInput"), "{ends:?}");
    let system = |i: usize| -> String {
        let requests = seen.lock().unwrap();
        let body: serde_json::Value = serde_json::from_str(requests[i].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        body["system"].as_str().unwrap().to_string()
    };
    assert!(!system(0).contains("Mirror what"));
    assert!(system(1).contains("docs/spec/constraints.md"), "the turn after the write names the file");
    assert!(system(2).contains("carries no doc id") && !system(2).contains("Mirror what"), "the reminder, for one turn: {}", system(2));
    assert!(!system(3).contains("carries no doc id"), "and gone");
    assert!(!entries.iter().any(|e| matches!(&e.event, Event::HookRan { outcome: event_kinds::HookOutcome::Blocked { .. }, .. })));
}

#[test]
fn a_project_is_asked_about_its_docs_once() {
    let root = temp_dir("ask");
    let data = temp_dir("ask-data");
    assert!(first_start_asks(&data, &root));
    assert!(!first_start_asks(&data, &root), "once");
    let other = temp_dir("ask2");
    std::fs::create_dir_all(format!("{other}/.harness")).unwrap();
    assert!(!first_start_asks(&data, &other), "it already has one of them");
}

/// ADR 0013: the agent or the user switches step-debug; its rules are there only while it is on.
#[test]
fn step_debug_is_switched_logged_and_replayed() {
    let root = temp_dir("sd");
    let data = temp_dir("sd-data");
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("d1", "debug_mode", json!({"on": true}))), "Switching."),
        anthropic_turn(Some(("d2", "debug_contract", json!({}))), "Reading the contract."),
        anthropic_turn(Some(("d3", "debug_runs", json!({}))), "Reading runs."),
        anthropic_turn(Some(("d4", "debug_mode", json!({}))), "Oops."),
        anthropic_turn(Some(("d5", "edit", json!({"path": "a.txt", "old_text": "a", "new_text": "b"}))), "It may still edit."),
        anthropic_turn(None, "Waiting."),
        anthropic_turn(Some(("d6", "debug_mode", json!({"on": false}))), "Done."),
        anthropic_turn(None, "Off."),
    ]);
    std::fs::write(format!("{root}/a.txt"), "a").unwrap();
    let mut s = settings(&url, "k");
    s.hooks = vec![HookSetting { event: Some("StepDebugEnter".into()), command: "echo debugging".into(), name: Some("on-debug".into()) }];
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    let (entries, _, _) = drive(&mut sessions, &["debug the pipeline"], Answer::AllowOnce);
    let result = |id: &str| entries.iter().find_map(|e| if let Event::ToolResult { id: i, output, is_error } = &e.event { (i == id).then(|| (output.clone(), *is_error)) } else { None }).unwrap();
    assert!(result("d2").0.contains("/debug/approve"), "{:?}", result("d2"));
    assert_eq!(result("d3"), ("[]".to_string(), false));
    assert!(!std::path::Path::new(&format!("{root}/.harness")).exists(), "reading runs creates nothing");
    assert!(result("d4").1);
    assert_eq!(std::fs::read_to_string(format!("{root}/a.txt")).unwrap(), "b", "the mode does not limit tools");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::HookRan { point: event_kinds::HookPointKind::StepDebugEnter, .. })));

    let listed = recent(&data, &root);
    let stored = read_log(&data, &root, &listed[0].id);
    assert!(SessionState::fold(stored.iter().map(|e| &e.event)).step_debug, "a replay says it is on");

    let (entries, _, _) = drive(&mut sessions, &["finish"], Answer::AllowOnce);
    assert!(entries.iter().any(|e| matches!(&e.event, Event::StepDebug { on: false, by } if by == "agent")));
    let requests: Vec<serde_json::Value> = seen.lock().unwrap().iter().map(|r| serde_json::from_str(r.split("\r\n\r\n").nth(1).unwrap()).unwrap()).collect();
    assert!(!requests[0]["system"].as_str().unwrap().contains("STEP-DEBUG"));
    assert!(requests[1]["system"].as_str().unwrap().contains("STEP-DEBUG"));
    assert!(!requests.last().unwrap()["system"].as_str().unwrap().contains("STEP-DEBUG"));
}

/// CON-55: a provider on a plan takes its token from the sign-in, and nothing writes it down.
#[test]
fn a_provider_on_a_plan_uses_the_sign_in_and_logs_no_token() {
    let root = temp_dir("plan-auth");
    let data = temp_dir("plan-auth-data");
    let store_dir = format!("{data}/auth/chatgpt");
    std::fs::create_dir_all(&store_dir).unwrap();
    let saved_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    std::fs::write(format!("{store_dir}/credentials.json"), json!({
        "email": null, "issuer": "http://127.0.0.1:1", "subject": "u", "client_id": "c", "ext_agent_host_id": "h", "id_token": "i",
        "access_token": "PLAN-ACCESS-TOKEN", "refresh_token": "PLAN-REFRESH-TOKEN", "token_type": "Bearer", "expires_in": 3600, "scopes": [], "saved_at_s": saved_at,
    }).to_string()).unwrap();
    let events = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hello\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n";
    let (url, seen) = fake_endpoint(vec![events.to_string()]);
    let mut s = Settings { provider: "chatgpt".into(), model: "some-model".into(), ..Default::default() };
    s.providers.insert("chatgpt".into(), json!({"kind": "openai-responses", "base_url": url}));
    s.auth.insert("chatgpt".into(), json!({"oauth": "chatgpt"}));
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    let (entries, ends, _) = drive(&mut sessions, &["hi"], Answer::Deny);
    assert_eq!(ends, ["AwaitingInput"], "{entries:?}");
    let request = seen.lock().unwrap()[0].clone();
    assert!(request.starts_with("POST /responses") || request.contains("/responses HTTP"), "{}", &request[..80]);
    assert!(request.to_lowercase().contains("authorization: bearer plan-access-token"), "the token was sent");
    assert!(!format!("{entries:?}").contains("PLAN-"), "no token in the log");
    let listed = recent(&data, &root);
    let stored = std::fs::read_to_string(format!("{data}/sessions/{}/{}.jsonl", root.chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect::<String>().trim_matches('-'), listed[0].id)).unwrap();
    assert!(!stored.contains("PLAN-"), "no token in the stored log");
}

/// CON-56: the empty adapter refuses, with its reason, and nothing is sent anywhere.
#[test]
fn the_claude_subscription_adapter_refuses_before_any_request() {
    let root = temp_dir("refused");
    let data = temp_dir("refused-data");
    let (url, seen) = fake_endpoint(vec![anthropic_turn(None, "must not run")]);
    let mut s = settings(&url, "k");
    s.auth.insert("anthropic".into(), json!({"oauth": "claude-subscription", "key": "ignored"}));
    let mut sessions = Sessions::new(real_factory(s, root.clone(), data.clone().into()), root.clone());
    let (entries, ends, _) = drive(&mut sessions, &["hi"], Answer::Deny);
    assert!(ends[0].contains("forbidden by Anthropic") || format!("{entries:?}").contains("forbidden by Anthropic"), "{ends:?} {entries:?}");
    assert!(seen.lock().unwrap().is_empty(), "no request left");
}

/// A model passes a file to search as readily as a folder.
#[test]
fn search_takes_a_file_a_folder_and_says_when_neither() {
    let root = temp_dir("search");
    let data = temp_dir("search-data");
    std::fs::create_dir_all(format!("{root}/sub")).unwrap();
    std::fs::write(format!("{root}/sub/a.txt"), "first\nthe needle here\n").unwrap();
    let (url, _) = fake_endpoint(vec![
        anthropic_turn(Some(("s1", "search", json!({"text": "needle", "path": "sub/a.txt"}))), "File."),
        anthropic_turn(Some(("s2", "search", json!({"text": "needle", "path": "sub"}))), "Folder."),
        anthropic_turn(Some(("s3", "search", json!({"text": "needle", "path": "no/such"}))), "Neither."),
        anthropic_turn(None, "Done."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    let (entries, _, _) = drive(&mut sessions, &["find it"], Answer::AllowOnce);
    let result = |id: &str| entries.iter().find_map(|e| if let Event::ToolResult { id: i, output, is_error } = &e.event { (i == id).then(|| (output.clone(), *is_error)) } else { None }).unwrap();
    assert!(result("s1").0.contains("sub/a.txt:2: the needle here") && !result("s1").1, "{:?}", result("s1"));
    assert!(result("s2").0.contains("sub/a.txt:2:"), "{:?}", result("s2"));
    assert!(result("s3").1 && result("s3").0.contains("no such"), "{:?}", result("s3"));
}

/// The agent names itself or is named; the log keeps it, and the model knows.
#[test]
fn the_agents_name_is_set_by_the_tool_or_the_user_and_replayed() {
    let root = temp_dir("nick");
    let data = temp_dir("nick-data");
    let (url, seen) = fake_endpoint(vec![
        anthropic_turn(Some(("n1", "set_nick", json!({"nick": "Vega"}))), "Naming."),
        anthropic_turn(Some(("n2", "set_nick", json!({"nick": "x\ny"}))), "Bad."),
        anthropic_turn(Some(("n3", "set_nick", json!({"nick": "a".repeat(33)}))), "Too long."),
        anthropic_turn(None, "Done."),
        anthropic_turn(None, "Fine."),
    ]);
    let mut sessions = Sessions::new(real_factory(settings(&url, "k"), root.clone(), data.clone().into()), root.clone());
    let (entries, _, asked) = drive(&mut sessions, &["pick a name"], Answer::Deny);
    assert_eq!(asked, 0, "no prompt for a name");
    let result = |id: &str| entries.iter().find_map(|e| if let Event::ToolResult { id: i, output, is_error } = &e.event { (i == id).then(|| (output.clone(), *is_error)) } else { None }).unwrap();
    assert!(!result("n1").1 && result("n1").0.contains("Vega"));
    assert!(result("n2").1 && result("n3").1);
    assert_eq!(entries.iter().filter(|e| matches!(e.event, Event::AgentName { .. })).count(), 1, "bad names change nothing");

    sessions.nick(1, "Orion".to_string());
    sessions.nick(1, "  ".to_string());
    let (entries, _, _) = drive(&mut sessions, &["and now"], Answer::Deny);
    assert!(entries.iter().any(|e| matches!(&e.event, Event::AgentName { name, by } if name == "Orion" && by == "user")), "{entries:?}");
    assert!(entries.iter().any(|e| matches!(&e.event, Event::Error { source, .. } if source == "nick")), "{entries:?}");
    let last = seen.lock().unwrap().last().unwrap().clone();
    assert!(last.contains("You go by the name Orion"), "the model knows");

    let listed = recent(&data, &root);
    let stored = read_log(&data, &root, &listed[0].id);
    assert_eq!(SessionState::fold(stored.iter().map(|e| &e.event)).agent_name.as_deref(), Some("Orion"), "a replay says so");
}
