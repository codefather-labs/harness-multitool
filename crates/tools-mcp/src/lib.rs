//! The MCP servers that Claude Code and Codex run, as tools of the harness's own loop (CON-201).
//!
//! [`discovery`] finds them in their files; [`client`] speaks MCP to one over stdio; the hub here
//! starts each once per process (the daemon), when it is first needed, and keeps it; [`McpTool`]
//! is one of a server's tools as a [`harness_api::Tool`], named `mcp__<server>__<tool>` as Claude
//! Code names it (CON-191). A server's list of tools is kept on disk, so a session knows its tools
//! without starting anything.

pub mod client;
pub mod discovery;

use client::{Called, Client, Failure, Listed};
use discovery::Server;
use harness_api::{BoxFuture, Capability, Tool, ToolContext, ToolOutput, ToolSpec};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// How long a server has to start and list its tools.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long one call may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(600);

/// How a server is started for a session: the window's environment, the project, and where the
/// harness keeps its lists and logs (`<data dir>/mcp`).
#[derive(Clone, Debug)]
pub struct Start {
    pub base: Option<Arc<BTreeMap<String, String>>>,
    pub root: String,
    pub dir: PathBuf,
}

/// A server and the project it runs for, as the hub keys it: `<name>-<hash>`, the hash of its
/// command, arguments, environment and project.
pub fn key(server: &Server, root: &str) -> String {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    for part in [server.command.as_str(), root].into_iter().chain(server.args.iter().map(String::as_str)) {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    for (k, v) in &server.env {
        hash.update(k.as_bytes());
        hash.update([1]);
        hash.update(v.as_bytes());
        hash.update([0]);
    }
    let digest = hash.finalize();
    format!("{}-{}", server.name, digest.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>())
}

fn list_file(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.json"))
}

/// The tools a server listed when it last ran, if it has.
pub fn cached(server: &Server, start: &Start) -> Option<Vec<Listed>> {
    let text = std::fs::read_to_string(list_file(&start.dir, &key(server, &start.root))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(v["tools"].as_array()?.iter().filter_map(Listed::from_json).collect())
}

fn keep(server: &Server, start: &Start, tools: &[Listed]) {
    let file = list_file(&start.dir, &key(server, &start.root));
    let text = serde_json::json!({"server": server.name, "tools": tools.iter().map(Listed::to_json).collect::<Vec<_>>()}).to_string();
    let staging = file.with_extension("partial");
    if std::fs::create_dir_all(&start.dir).is_ok() && std::fs::write(&staging, text).is_ok() {
        let _ = std::fs::rename(&staging, &file);
    }
}

type Slot = Arc<Mutex<Option<Client>>>;

fn slot(key: &str) -> Slot {
    static HUB: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    let mut hub = HUB.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    hub.entry(key.to_string()).or_default().clone()
}

/// The running client of a slot, started when it has none or its process ended; a new start
/// lists its tools again and keeps the list.
fn running<'a>(held: &'a mut Option<Client>, server: &Server, start: &Start) -> Result<&'a mut Client, String> {
    if held.as_mut().is_some_and(|c| !c.alive()) {
        *held = None;
    }
    if held.is_none() {
        let log = start.dir.join(format!("{}.log", key(server, &start.root)));
        let mut client = Client::start(&server.command, &server.args, &server.env, start.base.as_deref(), &start.root, Some(&log), START_TIMEOUT)?;
        let tools = client.list(START_TIMEOUT).map_err(|e| format!("listing the tools of {}: {e}", server.name))?;
        keep(server, start, &tools);
        *held = Some(client);
    }
    Ok(held.as_mut().expect("started above"))
}

/// A server's tools: from its list on disk, else from the server, started for it and kept. When
/// it has not listed them within `wait`, None, and it goes on in the background: its tools come
/// with a later session.
pub fn tools(server: &Server, start: &Start, wait: Duration) -> Result<Option<Vec<Listed>>, String> {
    if let Some(tools) = cached(server, start) {
        return Ok(Some(tools));
    }
    let (send, got) = std::sync::mpsc::channel();
    let (server_c, start_c) = (server.clone(), start.clone());
    std::thread::spawn(move || {
        let slot = slot(&key(&server_c, &start_c.root));
        let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
        let _ = send.send(running(&mut held, &server_c, &start_c).map(|_| ()));
    });
    match got.recv_timeout(wait) {
        Ok(Ok(())) => Ok(cached(server, start)),
        Ok(Err(e)) => Err(e),
        Err(_) => Ok(None),
    }
}

/// Calls `tool` of `server`, starting the server when it is not running. A call is never sent
/// twice, since a tool may have done its work before its server went away: a server gone during
/// a call is started again by the next one.
pub fn call(server: &Server, start: &Start, tool: &str, arguments: &Value) -> Result<Called, String> {
    let slot = slot(&key(server, &start.root));
    let mut held = slot.lock().unwrap_or_else(|e| e.into_inner());
    let client = running(&mut held, server, start)?;
    match client.call(tool, arguments, CALL_TIMEOUT) {
        Ok(called) => Ok(called),
        Err(Failure::Gone(why)) => {
            *held = None;
            Err(format!("{}: {why}", server.name))
        }
        Err(e) => Err(e.to_string()),
    }
}

/// One tool of a server, as the own loop calls it.
pub struct McpTool {
    pub server: Arc<Server>,
    pub tool: Listed,
    pub start: Start,
}

/// The name the model sees: `mcp__<server>__<tool>`.
pub fn tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{server}__{}", discovery::tool_safe(tool))
}

impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: tool_name(&self.server.name, &self.tool.name), description: self.tool.description.clone(), schema: self.tool.schema.clone() }
    }

    /// A call runs a program of the user's: asked as a command is, and 'always' kept per tool.
    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Command(format!("mcp:{}/{}", self.server.name, self.tool.name))])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let out = match call(&self.server, &self.start, &self.tool.name, input) {
            Ok(Called { text, is_error: false }) => ToolOutput::ok(text),
            Ok(Called { text, is_error: true }) => ToolOutput::error(text),
            Err(e) => ToolOutput::error(e),
        };
        Box::pin(std::future::ready(out))
    }

    /// Sent once found: a server may list hundreds (journal#1662, CON-201).
    fn deferred(&self) -> bool {
        true
    }
}
