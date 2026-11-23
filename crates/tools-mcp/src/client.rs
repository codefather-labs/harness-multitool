//! An MCP client over stdio (CON-201): JSON-RPC 2.0, one message per line, as the MCP
//! specification's stdio transport has it (revision 2025-06-18). The server is a child process,
//! ended with the program that started it.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// The revision of MCP this client speaks first.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// A tool a server lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

impl Listed {
    pub fn to_json(&self) -> Value {
        json!({"name": self.name, "description": self.description, "inputSchema": self.schema})
    }

    pub fn from_json(v: &Value) -> Option<Listed> {
        Some(Listed {
            name: v["name"].as_str()?.to_string(),
            description: v["description"].as_str().unwrap_or_default().to_string(),
            schema: if v["inputSchema"].is_object() { v["inputSchema"].clone() } else { json!({"type": "object"}) },
        })
    }
}

/// What a call came to: its text, and whether the server said it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Called {
    pub text: String,
    pub is_error: bool,
}

/// A running server.
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    next: u64,
    root: String,
}

/// How a request went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The server is gone or never answers: start it again.
    Gone(String),
    /// It answered with a JSON-RPC error.
    Refused(String),
    /// It did not answer in time; it may still be working.
    TimedOut(String),
    /// The user stopped the turn.
    Stopped,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Gone(m) | Failure::Refused(m) | Failure::TimedOut(m) => f.write_str(m),
            Failure::Stopped => f.write_str(harness_api::TOOL_STOPPED),
        }
    }
}

impl Client {
    /// Starts `command` in `cwd` with `base` as its whole environment (this process's when None)
    /// and `env` over it, then initializes it. What it writes to stderr goes to `log`.
    pub fn start(command: &str, args: &[String], env: &BTreeMap<String, String>, base: Option<&BTreeMap<String, String>>, cwd: &str, log: Option<&std::path::Path>, timeout: Duration) -> Result<Client, String> {
        let mut c = Command::new(command);
        c.args(args).current_dir(cwd).stdin(Stdio::piped()).stdout(Stdio::piped());
        if let Some(base) = base {
            c.env_clear();
            c.envs(base.iter());
        }
        c.envs(env.iter());
        let stderr = log.and_then(|path| {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
        });
        c.stderr(stderr.map_or_else(Stdio::null, Stdio::from));
        let mut child = c.spawn().map_err(|e| format!("starting {command}: {e}"))?;
        harness_platform_native::children::register(child.id());
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (send, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut client = Client { child, stdin, lines, next: 1, root: cwd.to_string() };
        let params = json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {"roots": {"listChanged": false}}, "clientInfo": {"name": "harness-multitool", "version": env!("CARGO_PKG_VERSION")}});
        client.request("initialize", params, timeout).map_err(|e| format!("initializing {command}: {e}"))?;
        client.notify("notifications/initialized", json!({})).map_err(|e| e.to_string())?;
        Ok(client)
    }

    /// Every tool it lists, page by page.
    pub fn list(&mut self, timeout: Duration) -> Result<Vec<Listed>, Failure> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let page = self.request("tools/list", params, timeout)?;
            tools.extend(page["tools"].as_array().into_iter().flatten().filter_map(Listed::from_json));
            cursor = page["nextCursor"].as_str().filter(|c| !c.is_empty()).map(String::from);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
    }

    /// Calls a tool; its content as text, images and other kinds named.
    pub fn call(&mut self, tool: &str, arguments: &Value, timeout: Duration) -> Result<Called, Failure> {
        let arguments = if arguments.is_object() { arguments.clone() } else { json!({}) };
        let result = self.request("tools/call", json!({"name": tool, "arguments": arguments}), timeout)?;
        Ok(Called { text: content_text(&result), is_error: result["isError"] == true })
    }

    /// Whether the process is still there.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn send(&mut self, message: &Value) -> Result<(), Failure> {
        let mut line = message.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).and_then(|_| self.stdin.flush()).map_err(|e| Failure::Gone(format!("the server is gone: {e}")))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), Failure> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// One request and its answer. What the server asks meanwhile is answered: `ping`, and
    /// `roots/list` with the project; anything else is refused.
    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, Failure> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let until = Instant::now() + timeout;
        loop {
            if harness_api::platform::stopping::asked() {
                let _ = self.notify("notifications/cancelled", json!({"requestId": id, "reason": "the user stopped the turn"}));
                return Err(Failure::Stopped);
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                let _ = self.notify("notifications/cancelled", json!({"requestId": id, "reason": "timed out"}));
                return Err(Failure::TimedOut(format!("{method} had no answer in {} s", timeout.as_secs())));
            }
            let line = match self.lines.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => return Err(Failure::Gone(format!("the server ended before it answered {method}"))),
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
            if let Some(asked) = message["method"].as_str() {
                if !message["id"].is_null() {
                    let answer = match asked {
                        "ping" => json!({"jsonrpc": "2.0", "id": message["id"], "result": {}}),
                        "roots/list" => json!({"jsonrpc": "2.0", "id": message["id"], "result": {"roots": [{"uri": file_uri(&self.root), "name": "project"}]}}),
                        _ => json!({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": format!("{asked} is not supported")}}),
                    };
                    self.send(&answer)?;
                }
                continue;
            }
            if message["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(Failure::Refused(error["message"].as_str().unwrap_or("an error").to_string()));
            }
            return Ok(message["result"].clone());
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        harness_platform_native::children::forget(self.child.id());
    }
}

fn file_uri(path: &str) -> String {
    let path = path.replace('\\', "/");
    if path.starts_with('/') { format!("file://{path}") } else { format!("file:///{path}") }
}

/// A result's content as text: each text part; an image or other part by what it is.
pub fn content_text(result: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for part in result["content"].as_array().into_iter().flatten() {
        match part["type"].as_str() {
            Some("text") => parts.push(part["text"].as_str().unwrap_or_default().to_string()),
            Some("resource") => parts.push(part["resource"]["text"].as_str().map(String::from).unwrap_or_else(|| format!("[resource {}]", part["resource"]["uri"].as_str().unwrap_or_default()))),
            Some("resource_link") => parts.push(format!("[resource {}]", part["uri"].as_str().unwrap_or_default())),
            Some(kind) => parts.push(format!("[{kind} {}]", part["mimeType"].as_str().unwrap_or_default())),
            None => {}
        }
    }
    if parts.is_empty()
        && let Some(structured) = result.get("structuredContent")
    {
        return structured.to_string();
    }
    parts.join("\n")
}
