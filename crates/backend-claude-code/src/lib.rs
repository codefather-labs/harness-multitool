//! Claude Code as a delegate agent (IF-17, ADR 0020, CON-57..61): the harness runs the unmodified
//! `claude` binary and logs what it does. It never reads, stores or passes its credentials: the
//! user signs in to Claude Code itself. Claude Code keeps its own prompt and tools, and the harness
//! only adds to them. Mode `oneshot`: one process per message, `--resume` between them (CON-58).

pub mod conversations;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use harness_api::event::Event;
use harness_api::*;
use serde_json::Value;

pub const NAME: &str = "claude-code";
/// What may run with nobody to ask, until the harness answers prompts itself (journal#209).
const READ_ONLY_TOOLS: &str = "Read,Glob,Grep";

/// What one line of the stream comes to.
#[derive(Debug, PartialEq)]
pub enum Mapped {
    Event(Event),
    End(Result<(), String>),
}

/// Turns stream-json lines into the log's events.
#[derive(Default)]
pub struct Mapper {
    turn: u32,
    started: bool,
    model: String,
    /// A text block's words came as partial messages already.
    streamed: bool,
    /// Its total so far; `None` when that is not known, as for a fork of someone else's session.
    cost_total: Option<u64>,
}

impl Mapper {
    pub fn new(first_turn: u32) -> Self {
        Mapper { turn: first_turn, cost_total: Some(0), ..Default::default() }
    }

    /// From what the session's log says so far. Its first total on a fork only sets the base.
    pub fn with_cost(mut self, total_micro_usd: u64, fork: bool) -> Self {
        self.cost_total = (!fork).then_some(total_micro_usd);
        self
    }

    fn turn_started(&mut self, out: &mut Vec<Mapped>) {
        out.push(Mapped::Event(Event::TurnStarted { turn: self.turn, provider: NAME.to_string(), model: self.model.clone() }));
    }

    pub fn line(&mut self, line: &str) -> Vec<Mapped> {
        let mut out = Vec::new();
        let Ok(e) = serde_json::from_str::<Value>(line) else { return out };
        match e["type"].as_str().unwrap_or_default() {
            "system" if e["subtype"] == "init" => {
                self.model = e["model"].as_str().unwrap_or_default().to_string();
                if let Some(id) = e["session_id"].as_str() {
                    out.push(Mapped::Event(Event::BackendSession { id: id.to_string() }));
                }
                if !self.started {
                    self.started = true;
                    self.turn_started(&mut out);
                }
            }
            "stream_event" => self.stream(&e["event"], &mut out),
            "assistant" => {
                for block in e["message"]["content"].as_array().into_iter().flatten() {
                    match block["type"].as_str().unwrap_or_default() {
                        "text" if !self.streamed => out.push(Mapped::Event(Event::TextDelta { text: block["text"].as_str().unwrap_or_default().to_string() })),
                        "tool_use" => {
                            let call = ToolCall { id: block["id"].as_str().unwrap_or_default().to_string(), name: block["name"].as_str().unwrap_or_default().to_string(), input: block["input"].clone() };
                            // The call is part of the conversation, as well as a card.
                            out.push(Mapped::Event(Event::ModelToolCall { call: call.clone() }));
                            out.push(Mapped::Event(Event::ToolRequested { call, capabilities: vec![] }));
                        }
                        _ => {}
                    }
                }
            }
            "user" => {
                for block in e["message"]["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "tool_result") {
                    out.push(Mapped::Event(Event::ToolResult {
                        id: block["tool_use_id"].as_str().unwrap_or_default().to_string(),
                        output: result_text(&block["content"]),
                        is_error: block["is_error"] == true,
                    }));
                }
            }
            "rate_limit_event" if e["rate_limit_info"]["status"] == "rejected" => {
                out.push(Mapped::Event(Event::Error { source: NAME.to_string(), message: "Claude Code's usage limit is reached".to_string() }));
            }
            "result" => {
                let u = &e["usage"];
                let count = |k: &str| u[k].as_u64().unwrap_or(0);
                let usage = Usage { input: count("input_tokens"), output: count("output_tokens"), cache_read: count("cache_read_input_tokens"), cache_write: count("cache_creation_input_tokens") };
                let stop = match e["stop_reason"].as_str() {
                    Some("max_tokens") => StopReason::MaxTokens,
                    Some("end_turn") | None => StopReason::EndTurn,
                    Some(other) => StopReason::Other(other.to_string()),
                };
                let total = e["total_cost_usd"].as_f64().map(|c| (c * 1e6).round() as u64);
                let cost_micro_usd = total.and_then(|total| Some(total.saturating_sub(self.cost_total?)));
                out.push(Mapped::Event(Event::TurnFinished { turn: self.turn, stop, usage, cost_micro_usd }));
                if let Some(total) = total {
                    self.cost_total = Some(total);
                    out.push(Mapped::Event(Event::BackendCost { total_micro_usd: total }));
                }
                let failed = e["is_error"] == true;
                out.push(Mapped::End(if failed { Err(e["result"].as_str().map_or_else(|| "Claude Code reported an error".to_string(), str::to_string)) } else { Ok(()) }));
            }
            _ => {}
        }
        out
    }

    fn stream(&mut self, event: &Value, out: &mut Vec<Mapped>) {
        match event["type"].as_str().unwrap_or_default() {
            "message_start" => self.streamed = false,
            "content_block_start" if event["content_block"]["type"] == "thinking" => out.push(Mapped::Event(Event::ThinkingDelta { text: String::new() })),
            "content_block_delta" => match event["delta"]["type"].as_str().unwrap_or_default() {
                "text_delta" => {
                    self.streamed = true;
                    out.push(Mapped::Event(Event::TextDelta { text: event["delta"]["text"].as_str().unwrap_or_default().to_string() }));
                }
                "thinking_delta" => {
                    if let Some(text) = event["delta"]["thinking"].as_str().filter(|t| !t.is_empty()) {
                        out.push(Mapped::Event(Event::ThinkingDelta { text: text.to_string() }));
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// Runs `claude` for one message.
pub struct ClaudeCode {
    binary: String,
    model: String,
    root: String,
    allowed_tools: String,
    /// The user's own settings, hooks, servers and environment, not an isolated child (CON-69).
    own_environment: bool,
    /// No prompts at all: the user's own choice, as they may run Claude Code themselves (CON-70).
    skip_permissions: bool,
    /// What it runs with instead of this process's environment (a window's, in the daemon).
    environment: Option<std::sync::Arc<std::collections::BTreeMap<String, String>>>,
}

impl ClaudeCode {
    /// Settings: `binary` (default `claude`), `allowed_tools` (a comma list, default read-only),
    /// `environment` (`isolated` by default, or `user`), `permissions` (`skip` to run with none).
    pub fn new(model: &str, root: &str, settings: &Value) -> Self {
        ClaudeCode {
            binary: settings["binary"].as_str().unwrap_or("claude").to_string(),
            model: model.to_string(),
            root: root.to_string(),
            allowed_tools: settings["allowed_tools"].as_str().unwrap_or(READ_ONLY_TOOLS).to_string(),
            own_environment: settings["environment"] == "user",
            skip_permissions: settings["permissions"] == "skip",
            environment: None,
        }
    }

    /// Runs with this environment instead of this process's.
    pub fn with_environment(mut self, environment: std::sync::Arc<std::collections::BTreeMap<String, String>>) -> Self {
        self.environment = Some(environment);
        self
    }

    /// The arguments of one run. The user's own hooks and servers stay out of it (CON-61).
    pub fn args(&self, request: &BackendRequest<'_>) -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages"].iter().map(|a| a.to_string()).collect();
        if !self.own_environment {
            args.extend(["--setting-sources", "", "--strict-mcp-config", "--mcp-config", r#"{"mcpServers":{}}"#].iter().map(|a| a.to_string()));
        }
        if self.skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        } else {
            args.extend(["--permission-prompts".to_string(), "none".to_string(), "--allowedTools".to_string(), self.allowed_tools.clone()]);
        }
        if !self.model.is_empty() {
            args.extend(["--model".to_string(), self.model.clone()]);
        }
        if let Some(id) = request.session {
            args.extend(["--resume".to_string(), id.to_string()]);
            if request.fork {
                args.push("--fork-session".to_string());
            }
        }
        if let Some(system) = request.system.filter(|s| !s.is_empty()) {
            args.extend(["--append-system-prompt".to_string(), system.to_string()]);
        }
        if request.plan_mode {
            args.extend(["--permission-mode".to_string(), "plan".to_string()]);
        }
        args
    }

    fn run_blocking(&self, request: &BackendRequest<'_>, sink: &mut dyn FnMut(Event) -> bool) -> PluginResult<BackendEnd> {
        let mut command = Command::new(&self.binary);
        command.args(self.args(request)).current_dir(&self.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Nothing of the session that started the harness passes to the child; only where its own
        // configuration lives, which is where the user signed in.
        if let Some(base) = &self.environment {
            command.env_clear();
            command.envs(base.iter());
        }
        let names: Vec<String> = match &self.environment {
            Some(base) => base.keys().cloned().collect(),
            None => std::env::vars_os().map(|(name, _)| name.to_string_lossy().into_owned()).collect(),
        };
        for name in names {
            // Its own environment keeps what the user's hooks need, all but what marks a session
            // of its own as already running.
            let drop = if self.own_environment { name == "CLAUDECODE" || name.starts_with("CLAUDE_CODE_") } else { name.starts_with("CLAUDE") && name != "CLAUDE_CONFIG_DIR" };
            if drop {
                command.env_remove(name);
            }
        }
        let mut child = command.spawn().map_err(|e| {
            let why = if e.kind() == std::io::ErrorKind::NotFound { "it is not installed or not on the PATH".to_string() } else { e.to_string() };
            PluginError::new(format!("could not start {}: {why}", self.binary))
        })?;
        // On stdin, so it is on no command line for another program's pkill -f to match.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(request.prompt.as_bytes());
        }
        let (tx, rx) = channel();
        let stdout = child.stdout.take().expect("piped above");
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut stderr = child.stderr.take().expect("piped above");
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        // A session going on with no total on record (just linked, or forked): its first total only sets the base.
        let unknown = request.fork || (request.session.is_some() && request.cost_total_micro_usd == 0);
        let mut mapper = Mapper::new(request.turn).with_cost(request.cost_total_micro_usd, unknown);
        let stop_now = |child: &mut std::process::Child| {
            let _ = child.kill();
            let _ = child.wait();
            Ok(BackendEnd::Stopped)
        };
        loop {
            if (request.stopped)() {
                return stop_now(&mut child);
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(line) => {
                    for mapped in mapper.line(&line) {
                        match mapped {
                            Mapped::Event(event) => {
                                if !sink(event) {
                                    return stop_now(&mut child);
                                }
                            }
                            Mapped::End(end) => {
                                let _ = child.wait();
                                return end.map(|()| BackendEnd::Done).map_err(PluginError::new);
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let status = child.wait().ok();
                    let tail: String = errors.join().unwrap_or_default().chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                    return Err(PluginError::new(format!("{} ended with no result ({}): {}", self.binary, status.map_or("unknown".to_string(), |s| s.to_string()), tail.trim())));
                }
            }
        }
    }
}

impl AgentBackend for ClaudeCode {
    fn name(&self) -> &str {
        NAME
    }

    fn run<'a>(&'a self, request: BackendRequest<'a>, sink: &'a mut dyn FnMut(Event) -> bool) -> BoxFuture<'a, PluginResult<BackendEnd>> {
        Box::pin(async move { self.run_blocking(&request, sink) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(lines: &[Value]) -> Vec<Mapped> {
        let mut m = Mapper::new(1);
        lines.iter().flat_map(|l| m.line(&l.to_string())).collect()
    }

    #[test]
    fn a_run_becomes_the_logs_events() {
        let got = map(&[
            json!({"type": "system", "subtype": "hook_started", "session_id": "s"}),
            json!({"type": "system", "subtype": "init", "session_id": "s1", "model": "m1", "tools": ["Read"]}),
            json!({"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "thinking"}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "thinking_delta", "thinking": "", "estimated_tokens": 5}}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": "Hi"}}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Hi"}, {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "a"}}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "content": "body"}]}}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed"}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "stop_reason": "end_turn", "total_cost_usd": 0.0125, "usage": {"input_tokens": 2, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 7, "output_tokens": 5}}),
        ]);
        let e = |x| Mapped::Event(x);
        assert_eq!(got, vec![
            e(Event::BackendSession { id: "s1".into() }),
            e(Event::TurnStarted { turn: 1, provider: NAME.into(), model: "m1".into() }),
            e(Event::ThinkingDelta { text: String::new() }),
            e(Event::TextDelta { text: "Hi".into() }),
            e(Event::ModelToolCall { call: ToolCall { id: "t1".into(), name: "Read".into(), input: json!({"file_path": "a"}) } }),
            e(Event::ToolRequested { call: ToolCall { id: "t1".into(), name: "Read".into(), input: json!({"file_path": "a"}) }, capabilities: vec![] }),
            e(Event::ToolResult { id: "t1".into(), output: "body".into(), is_error: false }),
            e(Event::TurnFinished { turn: 1, stop: StopReason::EndTurn, usage: Usage { input: 2, output: 5, cache_read: 40, cache_write: 7 }, cost_micro_usd: Some(12_500) }),
            e(Event::BackendCost { total_micro_usd: 12_500 }),
            Mapped::End(Ok(())),
        ], "text said by partial messages is not repeated");
    }

    #[test]
    fn a_denial_an_error_and_a_limit_show() {
        let got = map(&[
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "No partials: this text shows"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t", "is_error": true, "content": [{"type": "text", "text": "needs approval"}]}]}}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected"}}),
            json!({"type": "result", "is_error": true, "result": "Not logged in · Please run /login", "usage": {}}),
        ]);
        assert!(got.contains(&Mapped::Event(Event::TextDelta { text: "No partials: this text shows".into() })));
        assert!(got.contains(&Mapped::Event(Event::ToolResult { id: "t".into(), output: "needs approval".into(), is_error: true })));
        assert!(got.iter().any(|m| matches!(m, Mapped::Event(Event::Error { message, .. }) if message.contains("usage limit"))));
        assert_eq!(got.last(), Some(&Mapped::End(Err("Not logged in · Please run /login".into()))));
        assert!(map(&[json!("not an object")]).is_empty());
        let mut m = Mapper::new(1);
        assert!(m.line("not json").is_empty());
    }

    fn request<'a>(session: Option<&'a str>, system: Option<&'a str>, plan_mode: bool, stopped: &'a dyn Fn() -> bool) -> BackendRequest<'a> {
        BackendRequest { prompt: "secret prompt", session, system, plan_mode, fork: false, cost_total_micro_usd: 0, turn: 1, stopped }
    }

    #[test]
    fn the_arguments_keep_the_user_out_and_the_prompt_off_the_command_line() {
        let never = || false;
        let c = ClaudeCode::new("m1", "/r", &json!({}));
        let args = c.args(&request(None, None, false, &never));
        let at = |flag: &str| args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag}: {args:?}"));
        assert_eq!(args[at("--setting-sources") + 1], "");
        assert!(args.contains(&"--strict-mcp-config".to_string()));
        assert_eq!(args[at("--permission-prompts") + 1], "none");
        assert_eq!(args[at("--allowedTools") + 1], "Read,Glob,Grep");
        assert_eq!(args[at("--model") + 1], "m1");
        assert!(!args.iter().any(|a| a.contains("secret prompt") || a == "--resume" || a == "--bare" || a == "--permission-mode" || a == "--system-prompt"), "{args:?}");

        let args = c.args(&request(Some("s1"), Some("note"), true, &never));
        let at = |flag: &str| args.iter().position(|a| a == flag).unwrap();
        assert_eq!(args[at("--resume") + 1], "s1");
        assert_eq!(args[at("--append-system-prompt") + 1], "note");
        assert_eq!(args[at("--permission-mode") + 1], "plan");
    }

    #[test]
    fn a_conversation_from_elsewhere_is_forked_and_its_row_opens_what_it_needs() {
        let stopped = || false;
        let request = BackendRequest { prompt: "p", session: Some("s1"), system: None, plan_mode: false, fork: true, cost_total_micro_usd: 0, turn: 1, stopped: &stopped };
        let isolated = ClaudeCode::new("m", "/r", &serde_json::json!({})).args(&request);
        assert!(isolated.iter().any(|a| a == "--fork-session") && isolated.iter().any(|a| a == "--strict-mcp-config"));
        assert!(!isolated.iter().any(|a| a == "--dangerously-skip-permissions"), "never by default");
        let own = ClaudeCode::new("m", "/r", &serde_json::json!({"environment": "user", "permissions": "skip"})).args(&request);
        for gone in ["--setting-sources", "--strict-mcp-config", "--mcp-config", "--permission-prompts", "--allowedTools"] {
            assert!(!own.iter().any(|a| a == gone), "{gone}");
        }
        assert!(own.iter().any(|a| a == "--dangerously-skip-permissions"));
        let next = BackendRequest { fork: false, ..request };
        assert!(!ClaudeCode::new("m", "/r", &serde_json::json!({})).args(&next).iter().any(|a| a == "--fork-session"), "only the first message forks");
    }

    #[test]
    fn a_total_is_turned_into_what_this_turn_cost() {
        let result = |total: f64| json!({"type": "result", "total_cost_usd": total, "usage": {}}).to_string();
        let cost = |mapper: &mut Mapper, total: f64| {
            mapper.line(&result(total)).into_iter().find_map(|m| match m {
                Mapped::Event(Event::TurnFinished { cost_micro_usd, .. }) => Some(cost_micro_usd),
                _ => None,
            }).unwrap()
        };
        let mut fresh = Mapper::new(1);
        assert_eq!(cost(&mut fresh, 1.0), Some(1_000_000));
        let mut resumed = Mapper::new(2).with_cost(1_000_000, false);
        assert_eq!(cost(&mut resumed, 1.5), Some(500_000), "the difference, not the total");
        assert!(resumed.line(&result(1.5)).iter().any(|m| matches!(m, Mapped::Event(Event::BackendCost { total_micro_usd: 1_500_000 }))), "kept for the next process");
        let mut forked = Mapper::new(9).with_cost(0, true);
        assert_eq!(cost(&mut forked, 300.0), None, "the past was not spent here");
        assert_eq!(cost(&mut forked, 300.25), Some(250_000));
    }
}
