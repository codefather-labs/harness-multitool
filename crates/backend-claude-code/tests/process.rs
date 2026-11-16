//! The process side, against a fake `claude`: a shell script that records what it was given.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use harness_api::event::Event;
use harness_api::*;
use harness_backend_claude_code::ClaudeCode;
use serde_json::json;

const NEVER: fn() -> bool = || false;

struct Fake {
    dir: tempfile::TempDir,
}

/// One at a time: a fork in another test inherits the script's open write handle, and running it
/// then fails with "Text file busy".
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fake(body: &str) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("claude");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    Fake { dir }
}

impl Fake {
    fn backend(&self) -> ClaudeCode {
        ClaudeCode::new("m1", &self.dir.path().to_string_lossy(), &json!({"binary": self.dir.path().join("claude").to_string_lossy()}))
    }
}

fn run(backend: &ClaudeCode, stopped: &dyn Fn() -> bool) -> (Vec<Event>, PluginResult<BackendEnd>) {
    let mut events = Vec::new();
    let mut sink = |e: Event| {
        events.push(e);
        true
    };
    let request = BackendRequest { woken: false, prompt: "the prompt", session: Some("s9"), system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 3, stopped, next_message: &|| None };
    let end = pollster::block_on(backend.run(request, &mut sink));
    (events, end)
}

#[test]
fn it_reads_the_stream_gets_the_prompt_on_stdin_and_inherits_no_session() {
    let _serial = serial();
    let f = fake(r#"IFS= read -r line; printf "%s\n" "$line" > got-stdin
printf '%s\n' "$*" > got-args
env > got-env
echo '{"type":"system","subtype":"init","session_id":"s10","model":"m1"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}}'
echo '{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":2}}'"#);
    // SAFETY of the test: only this test in this file sets these, and they name no other test's data.
    unsafe {
        std::env::set_var("CLAUDECODE", "1");
        std::env::set_var("CLAUDE_CODE_SESSION_ID", "parent");
        std::env::set_var("CLAUDE_CONFIG_DIR", "/kept");
    }
    let (events, end) = run(&f.backend(), &NEVER);
    assert_eq!(end.unwrap(), BackendEnd::Done);
    assert!(events.contains(&Event::BackendSession { id: "s10".into() }), "{events:?}");
    assert!(events.contains(&Event::TextDelta { text: "Hello".into() }));
    assert!(matches!(events.iter().find(|e| matches!(e, Event::TurnStarted { .. })), Some(Event::TurnStarted { turn: 3, .. })));
    let read = |name: &str| std::fs::read_to_string(f.dir.path().join(name)).unwrap();
    assert!(read("got-stdin").contains("the prompt"), "as a line of the stream");
    let args = read("got-args");
    assert!(args.contains("--resume s9") && !args.contains("the prompt"), "{args}");
    let env = read("got-env");
    assert!(!env.contains("CLAUDECODE") && !env.contains("CLAUDE_CODE_SESSION_ID"), "{env}");
    assert!(env.contains("CLAUDE_CONFIG_DIR=/kept"), "where the user signed in stays");
}

#[test]
fn a_missing_binary_a_silent_exit_and_an_error_result_are_errors() {
    let _serial = serial();
    let gone = ClaudeCode::new("m", "/tmp", &json!({"binary": "/no/such/claude"}));
    let (_, end) = run(&gone, &NEVER);
    assert!(end.unwrap_err().to_string().contains("not installed"));

    let (_, end) = run(&fake("echo oops >&2; exit 3").backend(), &NEVER);
    let error = end.unwrap_err().to_string();
    assert!(error.contains("no result") && error.contains("oops"), "{error}");

    let (_, end) = run(&fake(r#"read -r _; echo '{"type":"result","is_error":true,"result":"Not logged in · Please run /login","usage":{}}'"#).backend(), &NEVER);
    assert!(end.unwrap_err().to_string().contains("Not logged in"));
}

#[test]
fn a_stop_ends_the_process_at_once() {
    let _serial = serial();
    let f = fake(r#"echo '{"type":"system","subtype":"init","session_id":"s","model":"m"}'; exec sleep 30"#);
    let start = std::time::Instant::now();
    let stopped = move || start.elapsed() > std::time::Duration::from_millis(300);
    let (_, end) = run(&f.backend(), &stopped);
    assert_eq!(end.unwrap(), BackendEnd::Stopped);
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
}

/// One process through a session's messages: a message during a run goes in at once, and the run
/// waits for what it sent to be answered.
#[test]
fn one_process_takes_the_messages_of_a_session_and_one_sent_during_a_run() {
    let _serial = serial();
    let f = fake(r#"echo $$ >> got-pids
echo '{"type":"system","subtype":"init","session_id":"s9","model":"m1"}'
n=0
while IFS= read -r line; do
  n=$((n+1))
  printf '%s\n' "$line" >> got-lines
  echo '{"type":"user","isReplay":true,"message":{"content":[{"type":"text","text":"echo"}]}}'
  if [ "$n" = 1 ] && [ ! -e waited ]; then
    touch waited
    sleep 1
    IFS= read -r second && { printf '%s\n' "$second" >> got-lines; echo '{"type":"user","isReplay":true,"message":{"content":[{"type":"text","text":"echo"}]}}'; }
  fi
  echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Answer"}}}'
  echo '{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1}}'
done"#);
    let backend = f.backend();
    let waiting = std::cell::RefCell::new(vec!["during the run".to_string()]);
    let next = || waiting.borrow_mut().pop();
    let mut sink = |_: Event| true;
    let request = BackendRequest { woken: false, prompt: "first", session: Some("s9"), system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 1, stopped: &NEVER, next_message: &next };
    assert!(matches!(pollster::block_on(backend.run(request, &mut sink)), Ok(BackendEnd::Done)));
    let pid = backend.own_process();
    assert!(pid.is_some(), "it stays");
    let request = BackendRequest { woken: false, prompt: "second", session: Some("s9"), system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 2, stopped: &NEVER, next_message: &|| None };
    assert!(matches!(pollster::block_on(backend.run(request, &mut sink)), Ok(BackendEnd::Done)));
    assert_eq!(backend.own_process(), pid, "the same one");
    let read = |name: &str| std::fs::read_to_string(f.dir.path().join(name)).unwrap();
    let lines = read("got-lines");
    assert!(lines.contains("first") && lines.contains("during the run") && lines.contains("second"), "{lines}");
    assert_eq!(read("got-pids").lines().count(), 1, "started once");
    // Plan mode is fixed at its start: a new one. And what is out of date goes.
    let request = BackendRequest { woken: false, prompt: "third", session: Some("s9"), system: None, plan_mode: true, fork: false, cost_total_micro_usd: 0, turn: 3, stopped: &NEVER, next_message: &|| None };
    assert!(matches!(pollster::block_on(backend.run(request, &mut sink)), Ok(BackendEnd::Done)));
    assert_eq!(read("got-pids").lines().count(), 2);
    backend.reset();
    assert_eq!(backend.own_process(), None);
}

/// CON-177: a process that is replaced takes its background tasks with it: the run on the new one
/// ends the old list; a process that goes on adds no such list.
#[test]
fn a_replaced_process_ends_its_background_tasks() {
    let _serial = serial();
    let empty = |events: &[Event]| events.iter().any(|e| matches!(e, Event::BackgroundTasks { tasks } if tasks.is_empty()));
    // One turn and out: the first process starts a background task, then exits.
    let f = fake(r#"echo '{"type":"system","subtype":"init","session_id":"s9","model":"m1"}'
IFS= read -r line
echo '{"type":"user","isReplay":true,"message":{"content":[{"type":"text","text":"echo"}]}}'
if [ ! -e started ]; then
  touch started
  echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1","description":"Run the memory watchdog in the background","task_type":"local_bash"}]}'
fi
echo '{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1}}'"#);
    let backend = f.backend();
    let (first, end) = run(&backend, &NEVER);
    assert!(matches!(end, Ok(BackendEnd::Done)), "{end:?}");
    assert!(first.iter().any(|e| matches!(e, Event::BackgroundTasks { tasks } if tasks.len() == 1)), "{first:?}");
    assert!(!empty(&first), "a first process ends nothing");
    std::thread::sleep(std::time::Duration::from_millis(200));
    let (second, end) = run(&backend, &NEVER);
    assert!(matches!(end, Ok(BackendEnd::Done)), "{end:?}");
    assert!(empty(&second), "the gone process's task is ended: {second:?}");

    // A process that goes on: no empty list.
    let g = fake(r#"echo '{"type":"system","subtype":"init","session_id":"s9","model":"m1"}'
while IFS= read -r line; do
  echo '{"type":"user","isReplay":true,"message":{"content":[{"type":"text","text":"echo"}]}}'
  echo '{"type":"result","is_error":false,"stop_reason":"end_turn","total_cost_usd":0.01,"usage":{"input_tokens":1,"output_tokens":1}}'
done"#);
    let backend = g.backend();
    let (_, _) = run(&backend, &NEVER);
    let (again, _) = run(&backend, &NEVER);
    assert!(!empty(&again), "the same process: nothing ended: {again:?}");
    backend.reset();
}
