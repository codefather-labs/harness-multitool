//! Claude Code as a delegate agent (IF-17, ADR 0020, CON-57..61): the harness runs the unmodified
//! `claude` binary and logs what it does. It never reads, stores or passes its credentials: the
//! user signs in to Claude Code itself. Claude Code keeps its own prompt and tools, and the harness
//! only adds to them. Mode `oneshot`: one process per message, `--resume` between them (CON-58).

pub mod conversations;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use harness_api::event::Event;
use harness_api::*;
use serde_json::{Value, json};

pub const NAME: &str = "claude-code";
/// What may run with nobody to ask, until the harness answers prompts itself (journal#209).
const READ_ONLY_TOOLS: &str = "Read,Glob,Grep";

/// What a slave's program is told it may do by itself: reading inside the folder. All else comes to
/// its prompt tool, where the harness decides by its own rules and its own plan mode (CON-108).
pub const SLAVE_TOOLS: &str = "Read(./**)";

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
    /// The usage of this turn's latest API call.
    last_call: Option<Value>,
    /// It has been told to stop: a tool it refuses now was stopped, not failed.
    interrupted: bool,
}

/// What a system line of its stream says about its background tasks.
fn news(e: &Value) -> Option<Event> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    match e["subtype"].as_str()? {
        "background_tasks_changed" => Some(Event::BackgroundTasks {
            tasks: e["tasks"].as_array()?.iter().map(|t| harness_api::event::BackgroundTask { id: text(&t["task_id"]), description: text(&t["description"]), kind: text(&t["task_type"]) }).collect(),
        }),
        "compact_boundary" => Some(Event::BackendCompacted),
        "task_notification" => Some(Event::BackgroundDone { id: text(&e["task_id"]), status: text(&e["status"]), summary: text(&e["summary"]) }),
        _ => None,
    }
}

/// A line that begins no turn: news, or its echo of something already seen.
fn only_news(line: &str) -> bool {
    let Ok(e) = serde_json::from_str::<Value>(line) else { return true };
    match e["type"].as_str().unwrap_or_default() {
        "system" => e["subtype"] != "init",
        "user" => e["isReplay"] == true,
        "rate_limit_event" | "control_response" => true,
        _ => false,
    }
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
            "system" if e["subtype"] != "init" => out.extend(news(&e).map(Mapped::Event)),
            "system" => {
                self.model = e["model"].as_str().unwrap_or_default().to_string();
                if let Some(id) = e["session_id"].as_str() {
                    out.push(Mapped::Event(Event::BackendSession { id: id.to_string() }));
                }
                if !self.started {
                    self.started = true;
                    self.turn_started(&mut out);
                }
            }
            "stream_event" => {
                if !self.started {
                    self.started = true;
                    self.turn_started(&mut out);
                }
                self.stream(&e["event"], &mut out);
            }
            "assistant" => {
                if e["message"]["usage"].is_object() {
                    self.last_call = Some(e["message"]["usage"].clone());
                }
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
                        output: if self.interrupted { harness_api::TOOL_STOPPED.to_string() } else { result_text(&block["content"]) },
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
                // A result sums the turn's calls; how full the context is, is the last call's prompt.
                let last = self.last_call.take().unwrap_or_else(|| u.clone());
                let held = |k: &str| last[k].as_u64().unwrap_or(0);
                let usage = Usage { input: held("input_tokens"), output: count("output_tokens"), cache_read: held("cache_read_input_tokens"), cache_write: held("cache_creation_input_tokens") };
                let stop = match e["stop_reason"].as_str() {
                    Some("max_tokens") => StopReason::MaxTokens,
                    Some("end_turn") | None => StopReason::EndTurn,
                    Some(other) => StopReason::Other(other.to_string()),
                };
                let total = e["total_cost_usd"].as_f64().map(|c| (c * 1e6).round() as u64);
                let cost_micro_usd = total.and_then(|total| Some(total.saturating_sub(self.cost_total?)));
                // What the model really holds, which only this says; before the turn is counted.
                if let Some(window) = e["modelUsage"].as_object().and_then(|m| m.values().filter_map(|v| v["contextWindow"].as_u64()).max()) {
                    out.push(Mapped::Event(Event::ContextWindow { tokens: window }));
                }
                out.push(Mapped::Event(Event::TurnFinished { turn: self.turn, stop, usage, cost_micro_usd }));
                if let Some(total) = total {
                    self.cost_total = Some(total);
                    out.push(Mapped::Event(Event::BackendCost { total_micro_usd: total }));
                }
                let failed = e["is_error"] == true;
                // What it answers next, to a message it took late, is a turn of its own.
                self.started = false;
                self.turn += 1;
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

/// Where the window says the user's own `.claude.json` is: their MCP servers are read from it
/// for a profile's delegate (CON-145).
pub const USERS_CLAUDE_JSON_VAR: &str = "MULTITOOL_USERS_CLAUDE_JSON";

/// The user's MCP servers for this project, from their `.claude.json`: the global ones, then the
/// project's own over them. The harness's own names are left out: its servers are given apart.
pub fn user_servers(claude_json: &str, root: &str) -> serde_json::Map<String, serde_json::Value> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(claude_json) else { return Default::default() };
    let mut servers = v["mcpServers"].as_object().cloned().unwrap_or_default();
    if let Some(project) = v["projects"][root]["mcpServers"].as_object() {
        servers.extend(project.clone());
    }
    servers.retain(|name, _| name != "harness" && name != "mode");
    servers
}

/// A file only its owner can read, written whole.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let staging = path.with_extension("partial");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&staging)?, text.as_bytes())?;
    std::fs::rename(&staging, path)
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
    /// An MCP tool that answers this program's permission prompts (CON-108), instead of nobody.
    prompt_tool: Option<String>,
    /// The harness holds plan mode for it: the program's own has instructions that win over ours.
    harness_plans: bool,
    /// The harness's own tools, as an MCP configuration.
    mcp: Option<String>,
    readable_dir: Option<String>,
    live: std::cell::RefCell<Option<Live>>,
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
            prompt_tool: settings["permission_prompt_tool"].as_str().map(str::to_string),
            harness_plans: settings["harness_plans"] == true,
            readable_dir: None,
            mcp: None,
            live: std::cell::RefCell::new(None),
        }
    }

    /// A folder outside the project its tools may read: the harness's own, nothing else of its data.
    pub fn with_readable_dir(mut self, dir: Option<String>) -> Self {
        self.readable_dir = dir;
        self
    }

    /// Gives it these MCP servers, on top of its own if it has its own environment.
    pub fn with_mcp(mut self, config: String) -> Self {
        self.mcp = Some(config);
        self
    }

    /// Runs with this environment instead of this process's.
    pub fn with_environment(mut self, environment: std::sync::Arc<std::collections::BTreeMap<String, String>>) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Whether this run goes under the program's own plan mode.
    fn program_plans(&self, request: &BackendRequest<'_>) -> bool {
        request.plan_mode && !self.harness_plans
    }

    /// The arguments of one run. The user's own hooks and servers stay out of it (CON-61).
    pub fn args(&self, request: &BackendRequest<'_>) -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--input-format", "stream-json", "--replay-user-messages", "--output-format", "stream-json", "--verbose", "--include-partial-messages"].iter().map(|a| a.to_string()).collect();
        if !self.own_environment {
            args.extend(["--setting-sources", "", "--strict-mcp-config"].iter().map(|a| a.to_string()));
        }
        if !self.own_environment || self.mcp.is_some() {
            args.extend(["--mcp-config".to_string(), self.mcp.clone().unwrap_or_else(|| r#"{"mcpServers":{}}"#.to_string())]);
        }
        // The browser tools, always and in every environment, tab or slave (CON-133): they work
        // under --strict-mcp-config too (journal#900).
        args.push("--chrome".to_string());
        if self.skip_permissions {
            args.push("--dangerously-skip-permissions".to_string());
        } else {
            match &self.prompt_tool {
                // Asking and planning are the harness's there: the program's own would come as prompts.
                // A user's own tab keeps the program's plan mode, whose ExitPlanMode comes to the user as a prompt;
                // a slave's planning and Telegram are the harness's (CON-108). Nobody can answer AskUserQuestion here.
                Some(tool) => args.extend([
                    "--permission-prompts".to_string(),
                    "host".to_string(),
                    "--permission-prompt-tool".to_string(),
                    tool.clone(),
                    "--disallowedTools".to_string(),
                    if self.harness_plans { "AskUserQuestion,EnterPlanMode,ExitPlanMode,mcp__harness__telegram_send" } else { "AskUserQuestion" }.to_string(),
                ]),
                None => args.extend(["--permission-prompts".to_string(), "none".to_string()]),
            }
            args.extend(["--allowedTools".to_string(), if self.mcp.is_some() { format!("{},mcp__harness__*,mcp__mode__*", self.allowed_tools) } else { self.allowed_tools.clone() }]);
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
        if self.program_plans(request) {
            args.extend(["--permission-mode".to_string(), "plan".to_string()]);
        }
        if let Some(dir) = &self.readable_dir {
            args.extend(["--add-dir".to_string(), dir.clone()]);
        }
        args
    }

    /// A variable of the environment it runs with.
    fn var(&self, name: &str) -> Option<String> {
        match &self.environment {
            Some(base) => base.get(name).cloned(),
            None => std::env::var(name).ok(),
        }.filter(|v| !v.is_empty())
    }

    /// The user's own MCP servers, for a profile whose Claude Code folder is not the user's
    /// (CON-145): written to a private file in that folder and given by its path, so no server's
    /// environment shows on a command line. None for an isolated run, or when the folder is the
    /// user's own, whose servers Claude Code reads itself.
    pub fn user_servers_file(&self) -> Option<std::path::PathBuf> {
        if !self.own_environment {
            return None;
        }
        let users = std::path::PathBuf::from(self.var(USERS_CLAUDE_JSON_VAR)?);
        let dir = std::path::PathBuf::from(self.var("CLAUDE_CONFIG_DIR")?);
        if users == dir.join(".claude.json") {
            return None;
        }
        let servers = user_servers(&std::fs::read_to_string(&users).ok()?, &self.root);
        let path = dir.join("harness-user-mcp.json");
        write_private(&path, &serde_json::json!({ "mcpServers": servers }).to_string()).ok()?;
        Some(path)
    }

    /// Its process for this run: the one it has if that can go on, or a new one from its form.
    fn start(&self, request: &BackendRequest<'_>) -> PluginResult<Live> {
        // In a scope of its own where systemd can make one (CON-166).
        let mut command = harness_platform_native::scope::agent_command(&self.binary, "claude");
        let mut args = self.args(request);
        if let Some(file) = self.user_servers_file() {
            args.extend(["--mcp-config".to_string(), file.to_string_lossy().into_owned()]);
        }
        command.args(args).current_dir(&self.root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Nothing of the session that started the harness passes to the child; only where its own
        // configuration lives, which is where the user signed in.
        if let Some(base) = &self.environment {
            command.env_clear();
            command.envs(base.iter());
            harness_platform_native::scope::keep_bus(&mut command, base);
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
        // Ended with this program if a run still holds it then (journal#1021).
        harness_platform_native::children::register(child.id());
        let stdin = child.stdin.take().expect("piped above");
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
        Ok(Live { child, stdin, lines, errors: Some(errors), session: request.session.map(str::to_string), plan_mode: self.program_plans(request), prompt_seen: false, echoes: false, peeked: Default::default() })
    }

    fn run_blocking(&self, request: &BackendRequest<'_>, sink: &mut dyn FnMut(Event) -> bool) -> PluginResult<BackendEnd> {
        let mut slot = self.live.borrow_mut();
        if request.woken {
            let Some(live) = slot.as_mut() else { return Ok(BackendEnd::Done) };
            live.prompt_seen = true;
            let mapper = Mapper::new(request.turn).with_cost(request.cost_total_micro_usd, request.cost_total_micro_usd == 0);
            return self.run_rest(slot, request, sink, mapper, None);
        }
        let goes_on = slot.as_mut().is_some_and(|live| {
            matches!(live.child.try_wait(), Ok(None)) && live.plan_mode == self.program_plans(request) && !request.fork && (request.session.is_none() || live.session.as_deref() == request.session)
        });
        if !goes_on {
            // A run stopped already (a shutdown) starts no delegate: that would be a model call
            // with this message, paid for and thrown away (journal#1034).
            if (request.stopped)() {
                return Ok(BackendEnd::Stopped);
            }
            let replaced = slot.is_some();
            *slot = Some(self.start(request)?);
            // The process it had took its background tasks with it (CON-177); the new one tells
            // of its own only when they change.
            if replaced {
                sink(Event::BackgroundTasks { tasks: Vec::new() });
            }
        }
        let live = slot.as_mut().expect("just set");
        live.prompt_seen = false;
        // On stdin, so it is on no command line for another program's pkill -f to match.
        // If it has died, what it said is read below.
        let _ = live.send(request.prompt);
        // A session going on with no total on record (just linked, or forked): its first total only sets the base.
        let unknown = request.fork || (request.session.is_some() && request.cost_total_micro_usd == 0);
        let mapper = Mapper::new(request.turn).with_cost(request.cost_total_micro_usd, unknown);
        self.run_rest(slot, request, sink, mapper, Some(request.prompt))
    }

    /// Reads a run to its end, feeding it what the user sends meanwhile.
    fn run_rest(&self, mut slot: std::cell::RefMut<'_, Option<Live>>, request: &BackendRequest<'_>, sink: &mut dyn FnMut(Event) -> bool, mut mapper: Mapper, unanswered: Option<&str>) -> PluginResult<BackendEnd> {
        let live = slot.as_mut().expect("set by the caller");
        // It may have died just as it was given this; then a new one gets it, once.
        let mut unanswered = unanswered;
        // Messages sent during the run and not yet seen coming back, which it answers before it is
        // done; and a result that may turn out not to be the last.
        let mut unechoed: std::collections::VecDeque<String> = Default::default();
        let mut end_held = None::<(Instant, Result<(), String>)>;
        // Asked to stop what it is doing, and when it should have said it heard.
        let mut interrupted = false;
        let mut heard_by: Option<Instant> = None;
        loop {
            if (request.stopped)() {
                // Asked again with messages still waiting: the answer to them is cancelled too.
                if interrupted && !unechoed.is_empty() {
                    *slot = None;
                    return Ok(BackendEnd::Stopped);
                }
                if heard_by.is_none() {
                    let _ = live.interrupt();
                    interrupted = true;
                    mapper.interrupted = true;
                    heard_by = Some(Instant::now() + INTERRUPT_WAIT);
                }
            }
            // It does not answer to that: it is ended instead.
            if heard_by.is_some_and(|by| Instant::now() >= by) {
                *slot = None;
                return Ok(stopped(&unechoed));
            }
            // Done first: what comes after this result is the next run's, from its start.
            if let Some((until, end)) = &end_held
                && (unechoed.is_empty() || Instant::now() >= *until)
            {
                if interrupted {
                    // Messages it never came to go with a new one; otherwise it simply stands.
                    if !unechoed.is_empty() {
                        *slot = None;
                    }
                    return Ok(stopped(&unechoed));
                }
                let end = end.clone();
                return end.map(|()| BackendEnd::Done).map_err(PluginError::new);
            }
            while let Some(text) = (request.next_message)() {
                let _ = live.send(&text);
                unechoed.push_back(text);
            }
            let line = match live.peeked.pop_front() {
                Some(line) => Ok(line),
                None => live.lines.recv_timeout(Duration::from_millis(100)),
            };
            match line {
                Ok(line) => {
                    unanswered = None;
                    let parsed: Option<Value> = serde_json::from_str(&line).ok();
                    if let Some(e) = &parsed {
                        // The first one coming back is this run's own prompt.
                        if e["isReplay"] == true && e["message"]["content"].as_array().is_some_and(|c| c.iter().any(|b| b["type"] == "text")) {
                            live.echoes = true;
                            if live.prompt_seen {
                                unechoed.pop_front();
                                // Taken after that result: the run goes on to the result of this one,
                                // a new piece of work that the stop was not about.
                                if end_held.take().is_some() {
                                    interrupted = false;
                                    mapper.interrupted = false;
                                }
                            }
                            live.prompt_seen = true;
                            continue;
                        }
                        if e["type"] == "control_response" {
                            heard_by = None;
                            continue;
                        }
                        if e["type"] == "system" && e["subtype"] == "init" && let Some(id) = e["session_id"].as_str() {
                            live.session = Some(id.to_string());
                        }
                    }
                    for mapped in mapper.line(&line) {
                        match mapped {
                            Mapped::Event(event) => {
                                if !sink(event) {
                                    *slot = None;
                                    return Ok(stopped(&unechoed));
                                }
                            }
                            // Before its prompt came back: the end of a turn before this run.
                            Mapped::End(_) if live.echoes && !live.prompt_seen => {}
                            // Stopped, it says it failed: that is what was asked of it, and no error.
                            Mapped::End(_) if interrupted => end_held = Some((Instant::now() + QUEUE_AFTER_STOP, Ok(()))),
                            Mapped::End(end) => {
                                end_held = Some((Instant::now() + LATE_ECHO, end));
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // It ended with messages it never took: a new one takes them from its form.
                    if let Some(prompt) = unanswered {
                        unechoed.push_front(prompt.to_string());
                    }
                    if !unechoed.is_empty() {
                        let session = live.session.clone().or_else(|| request.session.map(str::to_string));
                        let again = BackendRequest { session: session.as_deref(), fork: false, ..*request };
                        *slot = Some(self.start(&again)?);
                        let fresh = slot.as_mut().expect("just set");
                        let words: Vec<String> = unechoed.drain(..).collect();
                        let _ = fresh.send(&words.join("\n\n"));
                        return self.run_rest(slot, request, sink, mapper, None);
                    }
                    if let Some((_, end)) = end_held {
                        *slot = None;
                        return end.map(|()| BackendEnd::Done).map_err(PluginError::new);
                    }
                    let mut live = slot.take().expect("set above");
                    let status = live.child.wait().ok();
                    let stderr = live.errors();
                    let tail: String = stderr.chars().rev().take(400).collect::<Vec<_>>().into_iter().rev().collect();
                    return Err(PluginError::new(format!("{} ended with no result ({}): {}", self.binary, status.map_or("unknown".to_string(), |s| s.to_string()), tail.trim())));
                }
            }
        }
    }
}

/// A run stopped: messages it was given and never took go on to the next, or stopping would lose them.
fn stopped(unechoed: &std::collections::VecDeque<String>) -> BackendEnd {
    if unechoed.is_empty() { BackendEnd::Stopped } else { BackendEnd::StoppedBeforeMessages(unechoed.iter().cloned().collect()) }
}

/// How long it has to say it heard a stop, before it is ended instead.
const INTERRUPT_WAIT: Duration = Duration::from_secs(1);
/// How long after a stop it has to take what is queued: measured at 2 s on real Claude Code.
const QUEUE_AFTER_STOP: Duration = Duration::from_secs(10);

/// How long a result waits for a message sent just before it to come back.
const LATE_ECHO: Duration = Duration::from_secs(3);

/// How long it has to end by itself once asked.
#[cfg(unix)]
const ENDING: Duration = Duration::from_secs(3);

/// A running Claude Code, kept between the messages of a session.
struct Live {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    errors: Option<std::thread::JoinHandle<String>>,
    /// The conversation it holds.
    session: Option<String>,
    plan_mode: bool,
    /// This run's prompt has come back.
    prompt_seen: bool,
    /// It says when it takes a message.
    echoes: bool,
    /// Read between runs, for the run that follows.
    peeked: std::collections::VecDeque<String>,
}

impl Live {
    /// Asks it to stop what it is doing: its own way, which keeps the process and what it runs in
    /// the background, and it goes on to whatever it was given next.
    fn interrupt(&mut self) -> std::io::Result<()> {
        let line = json!({"type": "control_request", "request_id": format!("stop-{}", self.child.id()), "request": {"subtype": "interrupt"}});
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()
    }

    fn send(&mut self, text: &str) -> std::io::Result<()> {
        let line = json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]}});
        writeln!(self.stdin, "{line}")?;
        self.stdin.flush()
    }

    /// What it said on its error stream, once it has ended.
    fn errors(&mut self) -> String {
        self.errors.take().and_then(|handle| handle.join().ok()).unwrap_or_default()
    }
}

impl Drop for ClaudeCode {
    /// The session is over: its process goes with it.
    fn drop(&mut self) {
        *self.live.borrow_mut() = None;
    }
}

impl Drop for Live {
    /// Asked to end, it ends what it runs in the background; killed, it leaves that running with
    /// nobody knowing of it. So it is asked first.
    fn drop(&mut self) {
        harness_platform_native::children::forget(self.child.id());
        #[cfg(unix)]
        {
            let asked = std::process::Command::new("kill").args(["-TERM", &self.child.id().to_string()]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
            if asked.is_ok_and(|status| status.success()) {
                let end = Instant::now() + ENDING;
                while Instant::now() < end {
                    if !matches!(self.child.try_wait(), Ok(None)) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl AgentBackend for ClaudeCode {
    fn name(&self) -> &str {
        NAME
    }

    fn run<'a>(&'a self, request: BackendRequest<'a>, sink: &'a mut dyn FnMut(Event) -> bool) -> BoxFuture<'a, PluginResult<BackendEnd>> {
        Box::pin(async move { self.run_blocking(&request, sink) })
    }

    fn reset(&self) {
        *self.live.borrow_mut() = None;
    }

    fn own_process(&self) -> Option<u32> {
        self.live.borrow().as_ref().map(|live| live.child.id())
    }

    fn between_runs(&self, sink: &mut dyn FnMut(Event)) -> bool {
        let Ok(mut slot) = self.live.try_borrow_mut() else { return false };
        let Some(live) = slot.as_mut() else { return false };
        while let Ok(line) = live.lines.try_recv() {
            live.peeked.push_back(line);
        }
        while let Some(line) = live.peeked.front() {
            if !only_news(line) {
                return true;
            }
            if let Some(event) = serde_json::from_str::<Value>(line).ok().as_ref().and_then(news) {
                sink(event);
            }
            live.peeked.pop_front();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CON-145: the user's servers for this project, the project's own over the global ones, the
    /// harness's names left out; written private, and only for a run in the user's environment.
    #[test]
    fn the_users_servers_go_to_a_private_file_not_the_command_line() {
        let json = serde_json::json!({"mcpServers": {"a": {"command": "1"}, "b": {"command": "global"}, "harness": {}, "mode": {}}, "projects": {"/p": {"mcpServers": {"b": {"command": "project"}}}, "/q": {"mcpServers": {"c": {}}}}}).to_string();
        let servers = user_servers(&json, "/p");
        assert_eq!(servers.keys().cloned().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(servers["b"]["command"], "project");
        assert!(user_servers("not json", "/p").is_empty());
        let dir = tempfile::tempdir().unwrap();
        let users = dir.path().join("user.json");
        std::fs::write(&users, serde_json::json!({"mcpServers": {"a": {"command": "x", "env": {"T": "CANARY"}}}}).to_string()).unwrap();
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let env = |pairs: &[(&str, &std::path::Path)]| std::sync::Arc::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string_lossy().into_owned())).collect::<std::collections::BTreeMap<_, _>>());
        let own = ClaudeCode::new("m", "/p", &serde_json::json!({"environment": "user"})).with_environment(env(&[(USERS_CLAUDE_JSON_VAR, &users), ("CLAUDE_CONFIG_DIR", &profile)]));
        let file = own.user_servers_file().unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("CANARY"));
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&file).unwrap().permissions()) & 0o777, 0o600);
        let isolated = ClaudeCode::new("m", "/p", &serde_json::json!({})).with_environment(env(&[(USERS_CLAUDE_JSON_VAR, &users), ("CLAUDE_CONFIG_DIR", &profile)]));
        assert!(isolated.user_servers_file().is_none(), "an isolated run keeps --strict-mcp-config alone");
        let users_own = ClaudeCode::new("m", "/p", &serde_json::json!({"environment": "user"})).with_environment(env(&[(USERS_CLAUDE_JSON_VAR, &profile.join(".claude.json")), ("CLAUDE_CONFIG_DIR", &profile)]));
        assert!(users_own.user_servers_file().is_none(), "the user's own folder: Claude Code reads its servers itself");
    }

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
        BackendRequest { woken: false, prompt: "secret prompt", session, system, plan_mode, fork: false, cost_total_micro_usd: 0, turn: 1, stopped, next_message: &|| None }
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
        // The modes are allowed wherever the harness's own tools are (journal#541).
        let with_tools = ClaudeCode::new("m1", "/r", &json!({})).with_mcp("{}".into()).args(&request(None, None, false, &never));
        let allowed = &with_tools[with_tools.iter().position(|a| a == "--allowedTools").expect("allowed") + 1];
        assert!(allowed.contains("mcp__harness__*") && allowed.contains("mcp__mode__*"), "{allowed}");
        // A row with a prompt tool sends the prompts there, not to nobody (CON-108).
        let asking = ClaudeCode::new("m1", "/r", &json!({"permission_prompt_tool": "mcp__mode__permission_ask", "harness_plans": true})).args(&request(None, None, false, &never));
        let flag = |flag: &str| asking[asking.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag}: {asking:?}")) + 1].clone();
        assert_eq!(flag("--permission-prompts"), "host");
        assert_eq!(flag("--permission-prompt-tool"), "mcp__mode__permission_ask");
        assert!(flag("--disallowedTools").contains("AskUserQuestion") && flag("--disallowedTools").contains("telegram_send"));
        // The user's own tab asks the user (journal#814): its Telegram and plan mode stay its own.
        let users = ClaudeCode::new("m1", "/r", &json!({"permission_prompt_tool": "mcp__mode__permission_ask"})).args(&request(None, None, false, &never));
        assert_eq!(users[users.iter().position(|a| a == "--disallowedTools").expect("disallowed") + 1], "AskUserQuestion");
        // A slave's plan mode is the harness's: the program's own has instructions that win over ours.
        let held = ClaudeCode::new("m1", "/r", &json!({"harness_plans": true})).args(&request(None, None, true, &never));
        assert!(!held.iter().any(|a| a == "--permission-mode"), "{held:?}");
        let own = ClaudeCode::new("m1", "/r", &json!({})).args(&request(None, None, true, &never));
        assert!(own.iter().any(|a| a == "--permission-mode"), "a user's own conversation keeps it: {own:?}");
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
        let request = BackendRequest { woken: false, prompt: "p", session: Some("s1"), system: None, plan_mode: false, fork: true, cost_total_micro_usd: 0, turn: 1, stopped: &stopped, next_message: &|| None };
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

    /// CON-133: Claude Code always runs with --chrome: in its own environment and in the isolated
    /// one (a slave's), and no row turns it off.
    #[test]
    fn every_delegate_runs_with_the_browser_tools() {
        let never = || false;
        for row in [json!({}), json!({"environment": "user"}), json!({"harness_plans": true, "permission_prompt_tool": "mcp__mode__permission_ask"}), json!({"chrome": false}), json!({"environment": "user", "chrome": false})] {
            let args = ClaudeCode::new("m", "/r", &row).with_mcp("{}".into()).args(&request(None, None, false, &never));
            assert_eq!(args.iter().filter(|a| *a == "--chrome").count(), 1, "{row}: {args:?}");
        }
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
