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
