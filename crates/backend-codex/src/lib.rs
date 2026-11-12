//! Codex as a delegate agent (journal#913, the external contract in journal#918): the harness runs
//! the unmodified `codex` binary and logs what it does. It never reads, stores or passes its
//! credentials: the user signs in to Codex itself, in the profile's own Codex folder (CODEX_HOME).
//! One process per message: `codex exec --json`, then `codex exec resume <thread>` for the next.
//! The prompt goes on stdin, so it is on no command line.

pub mod conversations;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use harness_api::event::Event;
use harness_api::*;
use serde_json::Value;

pub const NAME: &str = "codex";

/// How long an asked-to-stop process has to end before it is killed.
const ENDING: Duration = Duration::from_secs(3);

/// What one line of `codex exec --json` comes to.
#[derive(Debug, PartialEq)]
pub enum Mapped {
    Event(Event),
    End(Result<(), String>),
}

/// Turns `codex exec --json` lines into the log's events.
#[derive(Default)]
pub struct Mapper {
    turn: u32,
    model: String,
    started: bool,
}

impl Mapper {
    pub fn new(turn: u32, model: &str) -> Mapper {
        Mapper { turn, model: model.to_string(), started: false }
    }

    fn start(&mut self, out: &mut Vec<Mapped>) {
        if !self.started {
            self.started = true;
            out.push(Mapped::Event(Event::TurnStarted { turn: self.turn, provider: NAME.to_string(), model: self.model.clone() }));
        }
    }

    /// One line; anything that is not JSON, or a kind it does not know, says nothing.
    pub fn line(&mut self, line: &str) -> Vec<Mapped> {
        let mut out = Vec::new();
        let Ok(e) = serde_json::from_str::<Value>(line) else { return out };
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match e["type"].as_str().unwrap_or_default() {
            "thread.started" => {
                if let Some(id) = e["thread_id"].as_str() {
                    out.push(Mapped::Event(Event::BackendSession { id: id.to_string() }));
                }
            }
            "turn.started" => self.start(&mut out),
            "item.started" | "item.updated" | "item.completed" => {
                self.start(&mut out);
                let item = &e["item"];
                let done = e["type"] == "item.completed";
                let id = text(&item["id"]);
                match item["type"].as_str().unwrap_or_default() {
                    "agent_message" if done => out.push(Mapped::Event(Event::TextDelta { text: text(&item["text"]) })),
                    "reasoning" if done => out.push(Mapped::Event(Event::ThinkingDelta { text: text(&item["text"]) })),
                    "command_execution" => {
                        // Its start is the call; its end the result. A call seen only completed gets both.
                        let call = ToolCall { id: id.clone(), name: "shell".to_string(), input: serde_json::json!({"command": item["command"]}) };
                        if e["type"] == "item.started" || done && !self.seen_start(&id) {
                            out.push(Mapped::Event(Event::ModelToolCall { call: call.clone() }));
                            out.push(Mapped::Event(Event::ToolRequested { call, capabilities: vec![] }));
                        }
                        if done {
                            let failed = item["exit_code"].as_i64().is_some_and(|c| c != 0) || item["status"] == "failed" || item["status"] == "declined";
                            out.push(Mapped::Event(Event::ToolResult { id: id.clone(), output: text(&item["aggregated_output"]), is_error: failed }));
                        }
                    }
                    "mcp_tool_call" => {
                        let name = format!("{}__{}", text(&item["server"]), text(&item["tool"]));
                        let call = ToolCall { id: id.clone(), name, input: item["arguments"].clone() };
                        if e["type"] == "item.started" || done && !self.seen_start(&id) {
                            out.push(Mapped::Event(Event::ModelToolCall { call: call.clone() }));
                            out.push(Mapped::Event(Event::ToolRequested { call, capabilities: vec![] }));
                        }
                        if done {
                            let output = if item["result"].is_null() { text(&item["error"]["message"]) } else { item["result"].to_string() };
                            out.push(Mapped::Event(Event::ToolResult { id: id.clone(), output, is_error: item["status"] == "failed" || !item["error"].is_null() }));
                        }
                    }
                    "file_change" if done => {
                        let call = ToolCall { id: id.clone(), name: "apply_patch".to_string(), input: serde_json::json!({"changes": item["changes"]}) };
                        out.push(Mapped::Event(Event::ModelToolCall { call: call.clone() }));
                        out.push(Mapped::Event(Event::ToolRequested { call, capabilities: vec![] }));
                        out.push(Mapped::Event(Event::ToolResult { id: id.clone(), output: item["changes"].to_string(), is_error: item["status"] == "failed" }));
                    }
                    "error" if done => out.push(Mapped::Event(Event::Error { source: NAME.to_string(), message: text(&item["message"]) })),
                    _ => {}
                }
                if e["type"] == "item.started" {
                    self.mark_started(&id);
                }
            }
            "turn.completed" => {
                self.start(&mut out);
                let u = &e["usage"];
                let n = |key: &str| u[key].as_u64().unwrap_or(0);
                // Its input counts what came from the cache too; the log counts them apart.
                let usage = Usage { input: n("input_tokens").saturating_sub(n("cached_input_tokens")), output: n("output_tokens") + n("reasoning_output_tokens"), cache_read: n("cached_input_tokens"), cache_write: n("cache_write_input_tokens") };
                out.push(Mapped::Event(Event::TurnFinished { turn: self.turn, stop: StopReason::EndTurn, usage, cost_micro_usd: None }));
                out.push(Mapped::End(Ok(())));
            }
            "turn.failed" => {
                let message = text(&e["error"]["message"]);
                out.push(Mapped::Event(Event::Error { source: NAME.to_string(), message: message.clone() }));
                out.push(Mapped::End(Err(message)));
            }
            "error" => out.push(Mapped::Event(Event::Error { source: NAME.to_string(), message: text(&e["message"]) })),
            _ => {}
        }
        out
    }

    fn seen_start(&self, id: &str) -> bool {
        STARTED.with(|s| s.borrow().contains(id))
    }

    fn mark_started(&self, id: &str) {
        STARTED.with(|s| s.borrow_mut().insert(id.to_string()));
    }
}

thread_local! {
    /// The item ids a run has seen start, so a completed one is not called twice.
    static STARTED: std::cell::RefCell<std::collections::HashSet<String>> = std::cell::RefCell::new(Default::default());
}

/// Codex as a session's backend.
pub struct Codex {
    binary: String,
    model: String,
    root: String,
    /// `read-only`, `workspace-write` (the default) or `danger-full-access`.
    sandbox: String,
    /// No approvals and no sandbox: the user's own choice only (CON-70).
    skip_permissions: bool,
    /// What it runs with instead of this process's environment (a window's, in the daemon).
    environment: Option<std::sync::Arc<std::collections::BTreeMap<String, String>>>,
    /// The harness's own tools, as an MCP configuration ({"mcpServers": {name: {command, args}}}).
    mcp: Option<String>,
    /// The process of the run now going on, to stop it.
    running: std::cell::RefCell<Option<u32>>,
    /// Its sign-in, checked once before its first run (journal#976).
    signed_in: std::cell::Cell<bool>,
}

impl Codex {
    /// Settings: `binary` (default `codex`), `sandbox` (default `workspace-write`), `permissions`
    /// (`skip` to run with no approvals and no sandbox).
    pub fn new(model: &str, root: &str, settings: &Value) -> Codex {
        Codex {
            binary: settings["binary"].as_str().unwrap_or("codex").to_string(),
            model: model.to_string(),
            root: root.to_string(),
            sandbox: settings["sandbox"].as_str().unwrap_or("workspace-write").to_string(),
            skip_permissions: settings["permissions"] == "skip",
            environment: None,
            mcp: None,
            running: Default::default(),
            signed_in: Default::default(),
        }
    }

    pub fn with_environment(mut self, environment: std::sync::Arc<std::collections::BTreeMap<String, String>>) -> Codex {
        self.environment = Some(environment);
        self
    }

    pub fn with_mcp(mut self, mcp: String) -> Codex {
        self.mcp = Some(mcp);
        self
    }

    /// The command line of one run. The prompt is never on it: `-` reads it from stdin.
    pub fn args(&self, request: &BackendRequest<'_>) -> Vec<String> {
        let mut args = vec!["exec".to_string(), "--json".to_string(), "--skip-git-repo-check".to_string()];
        if !self.model.is_empty() {
            args.extend(["-m".to_string(), self.model.clone()]);
        }
        if self.skip_permissions {
            args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        } else {
            // In plan mode nothing in the project may change.
            let sandbox = if request.plan_mode { "read-only" } else { self.sandbox.as_str() };
            args.extend(["-s".to_string(), sandbox.to_string()]);
        }
        // The harness's servers, for this run only: -c values are TOML, and a JSON string or array
        // of strings is TOML as well.
        if let Some(servers) = self.mcp.as_deref().and_then(|m| serde_json::from_str::<Value>(m).ok()).and_then(|m| m["mcpServers"].as_object().cloned()) {
            for (name, server) in servers {
                if let Some(command) = server["command"].as_str() {
                    args.extend(["-c".to_string(), format!("mcp_servers.{name}.command={}", Value::from(command))]);
                }
                if let Some(list) = server["args"].as_array() {
                    args.extend(["-c".to_string(), format!("mcp_servers.{name}.args={}", Value::from(list.clone()))]);
                }
                // Codex asks before an MCP tool call by default, and `codex exec` has no one to ask,
                // so it would refuse every call (journal#1069). These are the harness's own servers,
                // given for this run: their tools run unasked, as the Claude Code delegate's do. The
                // user's own servers, from their config.toml, keep the approval they set.
                args.extend(["-c".to_string(), format!("mcp_servers.{name}.default_tools_approval_mode=\"approve\"")]);
            }
        }
        if let Some(session) = request.session {
            args.extend(["resume".to_string(), session.to_string()]);
        }
        args.push("-".to_string());
        args
    }

    /// What goes on stdin: the prompt, with what the harness adds to Codex's instructions before
    /// the first message of a conversation (Codex has no option for that).
    fn stdin(&self, request: &BackendRequest<'_>) -> String {
        match (request.system, request.session) {
            (Some(system), None) if !system.is_empty() => format!("{system}\n\n{}", request.prompt),
            _ => request.prompt.to_string(),
        }
    }

    fn spawn(&self, request: &BackendRequest<'_>) -> PluginResult<Child> {
        let mut command = Command::new(&self.binary);
        command.args(self.args(request)).current_dir(&self.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Only where the user signed in passes from the window: its environment, CODEX_HOME in it.
        if let Some(base) = &self.environment {
            command.env_clear();
            command.envs(base.iter());
        }
        let mut child = command.spawn().map_err(|e| {
            let why = if e.kind() == std::io::ErrorKind::NotFound { "it is not installed or not on the PATH".to_string() } else { e.to_string() };
            PluginError::new(format!("could not start {}: {why}", self.binary))
        })?;
        // Ended with this program if a run still holds it then (journal#1021).
        harness_platform_native::children::register(child.id());
        let mut stdin = child.stdin.take().expect("piped above");
        let text = self.stdin(request);
        std::thread::spawn(move || {
            let _ = stdin.write_all(text.as_bytes());
        });
        Ok(child)
    }

    /// Whether Codex is signed in where it will run, by its own `codex login status`, once. A folder
    /// with no sign-in would otherwise answer every message with a run of retried 401s.
    fn check_signed_in(&self) -> PluginResult<()> {
        if self.signed_in.get() {
            return Ok(());
        }
        let mut command = Command::new(&self.binary);
        command.args(["login", "status"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        if let Some(base) = &self.environment {
            command.env_clear();
            command.envs(base.iter());
        }
        match command.status() {
            Ok(status) if status.success() => {
                self.signed_in.set(true);
                Ok(())
            }
            Ok(_) => {
                let place = self.environment.as_ref().and_then(|e| e.get("CODEX_HOME").cloned()).or_else(|| std::env::var("CODEX_HOME").ok()).unwrap_or_else(|| "~/.codex".to_string());
                Err(PluginError::new(format!("Codex is not signed in for this profile ({place}): sign in with Login on Settings > Provider, or `multitool login codex`")))
            }
            // A program that cannot run says so when it is started.
            Err(_) => Ok(()),
        }
    }

    fn run_blocking(&self, request: &BackendRequest<'_>, sink: &mut dyn FnMut(Event) -> bool) -> PluginResult<BackendEnd> {
        self.check_signed_in()?;
        STARTED.with(|s| s.borrow_mut().clear());
        // A run stopped already (a shutdown) starts no delegate (journal#1034).
        if (request.stopped)() {
            return Ok(BackendEnd::Stopped);
        }
        let mut child = self.spawn(request)?;
        *self.running.borrow_mut() = Some(child.id());
        let (tx, lines) = channel();
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
        let mut mapper = Mapper::new(request.turn, &self.model);
        let mut ended: Option<Result<(), String>> = None;
        let result = loop {
            if (request.stopped)() {
                end(&mut child);
                break Ok(BackendEnd::Stopped);
            }
            match lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    for mapped in mapper.line(&line) {
                        match mapped {
                            Mapped::Event(event) => {
                                if !sink(event) {
                                    end(&mut child);
                                    harness_platform_native::children::forget(child.id());
                                    *self.running.borrow_mut() = None;
                                    return Ok(BackendEnd::Stopped);
                                }
                            }
                            Mapped::End(end) => ended = Some(end),
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    let status = child.wait().ok();
                    let said = errors.join().unwrap_or_default();
                    break match ended.take() {
                        Some(Ok(())) => Ok(BackendEnd::Done),
                        Some(Err(message)) => Err(PluginError::new(message)),
                        None => Err(PluginError::new(match status {
                            Some(s) if s.success() => "codex ended without finishing its turn".to_string(),
                            _ => last_error(&said).unwrap_or_else(|| format!("codex failed ({})", status.map_or("no status".to_string(), |s| s.to_string()))),
                        })),
                    };
                }
            }
        };
        harness_platform_native::children::forget(child.id());
        *self.running.borrow_mut() = None;
        result
    }
}

/// The line of its stderr that says what went wrong, its own log lines aside.
fn last_error(stderr: &str) -> Option<String> {
    stderr.lines().rev().find(|l| l.starts_with("Error") || l.contains(" ERROR ")).map(|l| l.trim().to_string())
}

/// Asks the process to end, then kills it if it does not.
fn end(child: &mut Child) {
    #[cfg(unix)]
    {
        let asked = Command::new("kill").args(["-TERM", &child.id().to_string()]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        if asked.is_ok_and(|s| s.success()) {
            let until = Instant::now() + ENDING;
            while Instant::now() < until {
                if !matches!(child.try_wait(), Ok(None)) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl AgentBackend for Codex {
    fn name(&self) -> &str {
        NAME
    }

    fn own_process(&self) -> Option<u32> {
        *self.running.borrow()
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
        let mut m = Mapper::new(1, "gpt-x");
        lines.iter().flat_map(|l| m.line(&l.to_string())).collect()
    }

    /// The lines Codex 0.166.0 printed in the spike (journal#918), a shell call included.
    #[test]
    fn a_run_becomes_the_logs_events() {
        let got = map(&[
            json!({"type": "thread.started", "thread_id": "01a1cc4a-4dc6"}),
            json!({"type": "turn.started"}),
            json!({"type": "item.completed", "item": {"id": "item_0", "type": "agent_message", "text": "I'll read it."}}),
            json!({"type": "item.started", "item": {"id": "item_1", "type": "command_execution", "command": "/bin/bash -lc 'cat note.txt'", "aggregated_output": "", "exit_code": null, "status": "in_progress"}}),
            json!({"type": "item.completed", "item": {"id": "item_1", "type": "command_execution", "command": "/bin/bash -lc 'cat note.txt'", "aggregated_output": "spike\n", "exit_code": 0, "status": "completed"}}),
            json!({"type": "item.completed", "item": {"id": "item_2", "type": "agent_message", "text": "File content: spike"}}),
            json!({"type": "turn.completed", "usage": {"input_tokens": 44649, "cached_input_tokens": 39680, "cache_write_input_tokens": 0, "output_tokens": 71, "reasoning_output_tokens": 0}}),
        ]);
        let e = Mapped::Event;
        let call = ToolCall { id: "item_1".into(), name: "shell".into(), input: json!({"command": "/bin/bash -lc 'cat note.txt'"}) };
        assert_eq!(got, vec![
            e(Event::BackendSession { id: "01a1cc4a-4dc6".into() }),
            e(Event::TurnStarted { turn: 1, provider: NAME.into(), model: "gpt-x".into() }),
            e(Event::TextDelta { text: "I'll read it.".into() }),
            e(Event::ModelToolCall { call: call.clone() }),
            e(Event::ToolRequested { call, capabilities: vec![] }),
            e(Event::ToolResult { id: "item_1".into(), output: "spike\n".into(), is_error: false }),
            e(Event::TextDelta { text: "File content: spike".into() }),
            e(Event::TurnFinished { turn: 1, stop: StopReason::EndTurn, usage: Usage { input: 4969, output: 71, cache_read: 39680, cache_write: 0 }, cost_micro_usd: None }),
            Mapped::End(Ok(())),
        ]);
    }

    #[test]
    fn a_failed_command_a_failed_turn_and_noise() {
        let got = map(&[
            json!({"type": "item.completed", "item": {"id": "c", "type": "command_execution", "command": "false", "aggregated_output": "", "exit_code": 1, "status": "failed"}}),
            json!({"type": "turn.failed", "error": {"message": "usage limit reached"}}),
        ]);
        assert!(got.contains(&Mapped::Event(Event::ToolResult { id: "c".into(), output: String::new(), is_error: true })), "{got:?}");
        assert!(got.iter().any(|m| matches!(m, Mapped::Event(Event::ModelToolCall { .. }))), "a call seen only completed is still a call");
        assert_eq!(got.last(), Some(&Mapped::End(Err("usage limit reached".into()))));
        assert!(Mapper::new(1, "m").line("not json").is_empty());
        assert!(Mapper::new(1, "m").line(r#"{"type":"something.new"}"#).is_empty());
    }

    fn request<'a>(session: Option<&'a str>, plan_mode: bool, never: &'a dyn Fn() -> bool, none: &'a dyn Fn() -> Option<String>) -> BackendRequest<'a> {
        BackendRequest { prompt: "secret prompt", session, system: Some("note"), plan_mode, fork: false, cost_total_micro_usd: 0, turn: 1, stopped: never, next_message: none, woken: false }
    }

    #[test]
    fn the_command_line_keeps_the_prompt_off_and_resumes_by_id() {
        let (never, none) = (|| false, || None);
        let c = Codex::new("gpt-x", "/r", &json!({})).with_mcp(r#"{"mcpServers":{"harness":{"command":"/bin/mt","args":["mcp","--root","/r"]}}}"#.into());
        let first = c.args(&request(None, false, &never, &none));
        assert_eq!(&first[..3], ["exec", "--json", "--skip-git-repo-check"]);
        assert!(first.windows(2).any(|w| w == ["-m", "gpt-x"]) && first.windows(2).any(|w| w == ["-s", "workspace-write"]), "{first:?}");
        assert!(first.windows(2).any(|w| w == ["-c", r#"mcp_servers.harness.command="/bin/mt""#]), "{first:?}");
        assert!(first.windows(2).any(|w| w == ["-c", r#"mcp_servers.harness.args=["mcp","--root","/r"]"#]), "{first:?}");
        // journal#1069: codex exec has no one to ask, so the harness's own tools are approved.
        assert!(first.windows(2).any(|w| w == ["-c", r#"mcp_servers.harness.default_tools_approval_mode="approve""#]), "{first:?}");
        assert_eq!(first.iter().filter(|a| a.contains("default_tools_approval_mode")).count(), 1, "only the harness's own servers: {first:?}");
        assert_eq!(first.last().map(String::as_str), Some("-"));
        assert!(!first.iter().any(|a| a.contains("secret prompt") || a == "resume"));
        let next = c.args(&request(Some("t1"), true, &never, &none));
        assert!(next.windows(3).any(|w| w == ["resume", "t1", "-"]), "{next:?}");
        assert!(next.windows(2).any(|w| w == ["-s", "read-only"]), "plan mode changes nothing: {next:?}");
        assert_eq!(c.stdin(&request(None, false, &never, &none)), "note\n\nsecret prompt", "the harness's note before the first message");
        assert_eq!(c.stdin(&request(Some("t1"), false, &never, &none)), "secret prompt");
        let skip = Codex::new("m", "/r", &json!({"permissions": "skip"})).args(&request(None, false, &never, &none));
        assert!(skip.iter().any(|a| a == "--dangerously-bypass-approvals-and-sandbox") && !skip.iter().any(|a| a == "-s"), "{skip:?}");
    }

    /// journal#976: a Codex not signed in where it runs is refused once, in words, before any run;
    /// none of its retried 401s.
    #[cfg(unix)]
    #[test]
    fn a_codex_not_signed_in_is_refused_in_words() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex");
        let ran = dir.path().join("ran");
        std::fs::write(&fake, format!("#!/bin/sh\n[ \"$1 $2\" = \"login status\" ] && exit 1\ntouch {}\n", ran.display())).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = Codex::new("m", &dir.path().to_string_lossy(), &json!({"binary": fake.to_string_lossy()}))
            .with_environment(std::sync::Arc::new([("CODEX_HOME".to_string(), "/p/codex".to_string()), ("PATH".to_string(), "/usr/bin:/bin".to_string())].into_iter().collect()));
        let (never, none) = (|| false, || None);
        let req = BackendRequest { prompt: "hi", session: None, system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 1, stopped: &never, next_message: &none, woken: false };
        let end = pollster::block_on(c.run(req, &mut |_| true));
        let message = end.unwrap_err().to_string();
        assert!(message.contains("not signed in") && message.contains("/p/codex") && message.contains("multitool login codex"), "{message}");
        assert!(!ran.exists(), "no run was started");
    }

    /// A fake codex: reads its prompt from stdin and prints a run's lines; a second process resumes.
    #[cfg(unix)]
    #[test]
    fn a_real_process_runs_a_turn_and_is_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("codex");
        std::fs::write(&fake, "#!/bin/sh\nprompt=$(cat)\nif [ \"$prompt\" = slow ]; then sleep 30; fi\necho '{\"type\":\"thread.started\",\"thread_id\":\"T9\"}'\necho '{\"type\":\"turn.started\"}'\nprintf '{\"type\":\"item.completed\",\"item\":{\"id\":\"i\",\"type\":\"agent_message\",\"text\":\"heard %s\"}}\\n' \"$prompt\"\necho '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = Codex::new("m", &dir.path().to_string_lossy(), &json!({"binary": fake.to_string_lossy()}));
        let (never, none) = (|| false, || None);
        let mut seen = Vec::new();
        let req = BackendRequest { prompt: "hello", session: None, system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 1, stopped: &never, next_message: &none, woken: false };
        let end = pollster::block_on(c.run(req, &mut |e| {
            seen.push(e);
            true
        }));
        assert_eq!(end, Ok(BackendEnd::Done));
        assert!(seen.contains(&Event::BackendSession { id: "T9".into() }));
        assert!(seen.contains(&Event::TextDelta { text: "heard hello".into() }), "{seen:?}");
        let stop_at = Instant::now() + Duration::from_millis(300);
        let stopped = move || Instant::now() > stop_at;
        let req = BackendRequest { prompt: "slow", session: Some("T9"), system: None, plan_mode: false, fork: false, cost_total_micro_usd: 0, turn: 2, stopped: &stopped, next_message: &none, woken: false };
        let began = Instant::now();
        let end = pollster::block_on(c.run(req, &mut |_| true));
        assert_eq!(end, Ok(BackendEnd::Stopped));
        assert!(began.elapsed() < Duration::from_secs(5), "a stop ends it: {:?}", began.elapsed());
        assert!(c.own_process().is_none());
    }
}
