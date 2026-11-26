//! What the session's integration tests share: a fake provider endpoint, canned turns, temporary
//! folders, settings. Each test binary takes what it needs.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harness_api::*;
use harness_session::*;
use serde_json::json;

/// Serves one canned SSE body per request and records the requests.
pub fn fake_endpoint(bodies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
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

pub fn sse(events: &[serde_json::Value]) -> String {
    events.iter().map(|e| format!("event: {}\ndata: {e}\n\n", e["type"].as_str().unwrap_or("x"))).collect()
}

pub fn anthropic_turn(tool: Option<(&str, &str, serde_json::Value)>, text: &str) -> String {
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

/// The real factory, with this process's environment less what a harness running these tests sets for
/// its own profile: a test reads no one's real sign-in (journal#786).
pub fn hermetic_factory(settings: Settings, root: String, data_dir: std::path::PathBuf) -> harness_session::Factory {
    let mut env = harness_session::this_environment();
    for name in [harness_session::CHATGPT_DIR_VAR, "CLAUDE_CONFIG_DIR", "MULTITOOL_HOME"] {
        env.remove(name);
    }
    let host = harness_session::Host { environment: Some(std::sync::Arc::new(env)), ..Default::default() };
    harness_session::with_host(settings, root, data_dir, host)
}

/// A test's own folder. Folders of runs whose process is gone are removed first: the test harness
/// has no hook after the last test, so at most the last run's are left on the disk.
pub fn temp_dir(name: &str) -> String {
    // macOS's temp folder is long (/var/folders/…/T), and a daemon's socket under it would pass the
    // 104 bytes of a socket address; it is also a link to /private/var, and a program's own working
    // folder is the resolved path. /private/tmp is short and already resolved (journal#1584).
    let base = if cfg!(target_os = "macos") { std::path::PathBuf::from("/private/tmp") } else { std::env::temp_dir() };
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| {
        for entry in std::fs::read_dir(&base).into_iter().flatten().flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            let Some(pid) = file.strip_prefix("harness-e2e-").and_then(|rest| rest.rsplit('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
            if pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    });
    let dir = base.join(format!("harness-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

/// Runs prompts, answering every permission prompt with `answer`, until each run ends.
pub fn drive(sessions: &mut Sessions, prompts: &[&str], answer: Answer) -> (Vec<LogEntry>, Vec<RunEndOf>, u32) {
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
                FromSession::Synced { .. } | FromSession::Progress { .. } | FromSession::AskWithdrawn { .. } => {}
                FromSession::Queued { .. } => {}
                FromSession::Spawn { .. } | FromSession::CloseTab { .. } | FromSession::Room { .. } => {}
            }
        }
    }
    (entries, ends, asked)
}

pub type RunEndOf = String;

pub fn settings(url: &str, data_key: &str) -> Settings {
    let mut s = Settings::default();
    s.providers.insert("anthropic".into(), json!({"base_url": url, "fallbacks": true}));
    s.auth.insert("anthropic".into(), json!({"env": "HARNESS_E2E_NO_SUCH_VAR", "key": data_key}));
    s
}
