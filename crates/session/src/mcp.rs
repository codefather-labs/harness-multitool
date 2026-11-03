//! The harness's tools for an agent that runs on its own, over MCP: a server on standard input and
//! output, one JSON message a line. Claude Code starts one per session. The journal and maps it
//! serves are the daemon's (CON-20).

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::rc::Rc;

use harness_api::*;
use harness_daemon::client::Client;
use serde_json::{Value, json};

/// Who it serves.
pub struct Serving {
    pub data_dir: PathBuf,
    pub root: String,
    /// The daemon's window and tab of the session, for what is asked of the window.
    pub window: Option<(String, u64)>,
    /// `mode`: this server offers the harness's modes alone, under their short names, so a program
    /// sees them apart as `mcp__mode__*` (CON-96). Otherwise the utilities alone.
    pub namespace: Option<String>,
    /// The `map_max_nodes` setting as the config file holds it when this server starts (CON-114).
    pub map_max_nodes: u64,
}

/// Asks the window to rebuild and swap the harness, through the daemon.
struct Reload {
    client: Client,
    window: String,
    tab: u64,
}

impl Tool for Reload {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "plugins_reload".into(),
            description: "Rebuild this harness from its source and restart it, with this conversation. It builds now and swaps when your turn ends; then you get a message saying so, or saying why the build failed, and you go on.".into(),
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Command("rebuild and restart the harness".into())])
    }

    fn execute<'a>(&'a self, _input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match self.client.conversation_op(&json!({"window": self.window, "op": "reload", "tab": self.tab})) {
            Ok(_) => ToolOutput::ok("Building. The swap comes when this turn ends: end your turn when you have nothing else to do."),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}

/// The agent names itself: the name it goes by above its messages and in the Telegram channel.
struct SetNick {
    client: Client,
    window: String,
    tab: u64,
}

impl Tool for SetNick {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "set_nick".into(),
            description: "Choose the name you go by: above your messages, and the one /switch in Telegram finds you by.".into(),
            schema: json!({"type": "object", "properties": {"nick": {"type": "string"}}, "required": ["nick"]}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match input["nick"].as_str().map(str::trim).filter(|nick| !nick.is_empty()) {
            None => ToolOutput::error("nick is required"),
            Some(nick) => match self.client.conversation_op(&json!({"window": self.window, "op": "nick", "tab": self.tab, "text": nick})) {
                Ok(_) => ToolOutput::ok(format!("You go by {nick} now.")),
                Err(error) => ToolOutput::error(error),
            },
        };
        Box::pin(std::future::ready(answer))
    }
}

const MODE_PREFIX: &str = "mode__";

/// What an agent on the modes' server reads before its tools.
const MODE_INSTRUCTIONS: &str = "Modes of the harness, not utilities: entering one changes how you are run. plan_enter and plan_exit for studying and planning before anything is built; debug for reproducing a fix and verifying it; orchestrate makes you the master of agents you start, and the agent_ tools here work only in that mode. Where the agent workflow specification says mode__x, on this server it is mcp__mode__x.";

/// What an agent on this server reads before its tools (CON-96).
const INSTRUCTIONS: &str = "The harness you run under: its project journal, maps and flowcharts, Telegram, your name, other agents, and rebuilding it. These are utilities: each does one thing and answers. Read the journal's agenda on a fresh context. The harness's modes are on the server named mode. docs/tools.md in the harness's repository documents every tool and mode.";

fn tools(serving: &Serving) -> Result<Vec<Rc<dyn Tool>>, String> {
    let client = Client::find(&serving.data_dir);
    let memory: Rc<dyn Memory> = match &client {
        Some(client) => Rc::new(harness_daemon::memory::DaemonMemory::new(client.clone(), serving.data_dir.clone(), &serving.root, "multitool")),
        None => Rc::new(crate::project_memory(&serving.data_dir.to_string_lossy(), &serving.root)?),
    };
    let mut out: Vec<Rc<dyn Tool>> = Vec::new();
    for name in harness_tools_memory::NAMES.into_iter().chain(harness_tools_memory::MORE) {
        if let Some(tool) = harness_tools_memory::tool(memory.clone(), name, serving.map_max_nodes) {
            out.push(Rc::new(tool));
        }
    }
    out.push(Rc::new(crate::host_tools::TelegramSend { data_dir: serving.data_dir.clone(), from: serving.window.clone() }));
    if let (Some(client), Some((window, tab))) = (client, &serving.window) {
        for name in crate::host_tools::DELEGATE_MODE_TOOLS.into_iter().chain(crate::host_tools::MODE_TOOLS) {
            out.push(Rc::new(crate::host_tools::ModeTool { name, client: client.clone(), window: window.clone(), tab: *tab }));
        }
        out.push(Rc::new(crate::host_tools::PermissionAsk { client: client.clone(), window: window.clone(), tab: *tab }));
        out.push(Rc::new(crate::host_tools::AgentList { client: client.clone(), window: window.clone() }));
        out.push(Rc::new(crate::host_tools::AgentSend { client: client.clone(), window: window.clone(), tab: *tab }));
        out.push(Rc::new(SetNick { client: client.clone(), window: window.clone(), tab: *tab }));
        out.push(Rc::new(Reload { client, window: window.clone(), tab: *tab }));
    }
    // Of the two servers, each offers its own.
    let in_mode = serving.namespace.as_deref() == Some("mode");
    out.retain(|tool| tool.spec().name.starts_with(MODE_PREFIX) == in_mode);
    Ok(out)
}

/// Answers one message; `None` for a notification, which is not answered.
fn answer(message: &Value, tools: &[Rc<dyn Tool>], serving: &Serving) -> Option<Value> {
    let id = message.get("id")?.clone();
    let params = &message["params"];
    let result = match message["method"].as_str().unwrap_or_default() {
        "initialize" => json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "harness", "version": env!("CARGO_PKG_VERSION")},
            "instructions": if serving.namespace.as_deref() == Some("mode") { MODE_INSTRUCTIONS } else { INSTRUCTIONS },
        }),
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = tools.iter().map(|t| t.spec()).map(|s| json!({"name": s.name.strip_prefix(MODE_PREFIX).unwrap_or(&s.name), "description": s.description, "inputSchema": s.schema})).collect();
            json!({"tools": list})
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let Some(tool) = tools.iter().find(|t| t.spec().name.strip_prefix(MODE_PREFIX).unwrap_or(&t.spec().name) == name) else {
                return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("no tool {name}")}}));
            };
            let input = if params["arguments"].is_object() { params["arguments"].clone() } else { json!({}) };
            // What the program itself sends is its own to shape.
            // An agent's task is its master's to close (CON-109). A daemon that cannot say refuses nothing here.
            if name == "journal_close" || name == "journal_abandon" {
                let task = serving.window.as_ref().and_then(|(window, tab)| {
                    let client = Client::find(&serving.data_dir)?;
                    client.conversation_op(&json!({"window": window, "op": "slave_task", "tab": tab})).ok()?["task"].as_u64()
                });
                if harness_core::engine::closes_task(name, &input, task) {
                    return Some(json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": harness_core::engine::TASK_IS_THE_MASTERS}], "isError": true}}));
                }
            }
            let unknown = harness_core::engine::unknown_arguments(&tool.spec().schema, &input);
            if !unknown.is_empty() && name != "permission_ask" {
                let text = format!("invalid arguments: {name} has no argument {}", unknown.join(", "));
                return Some(json!({"jsonrpc": "2.0", "id": id, "result": {"content": [{"type": "text", "text": text}], "isError": true}}));
            }
            // What it may do was the agent's own permissions to settle.
            let cx = ToolContext { platform: harness_platform_native::platform(serving.data_dir.clone()), granted: tool.capabilities(&input).unwrap_or_default(), root: serving.root.clone() };
            let output = pollster::block_on(tool.execute(&input, &cx));
            json!({"content": [{"type": "text", "text": output.text}], "isError": output.is_error})
        }
        other => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("no method {other}")}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// Serves until its input ends.
pub fn serve(serving: &Serving, input: impl BufRead, mut output: impl Write) -> Result<(), String> {
    let tools = tools(serving)?;
    for line in input.lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(reply) = answer(&message, &tools, serving) {
            writeln!(output, "{reply}").map_err(|e| e.to_string())?;
            output.flush().map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// How Claude Code is told to start it for this tab.
pub fn config(program: &std::path::Path, root: &str, window: Option<&str>, tab: u64) -> String {
    let mut args = vec![json!("mcp"), json!("--root"), json!(root)];
    if let Some(window) = window {
        args.extend([json!("--window"), json!(window), json!("--tab"), json!(tab.to_string())]);
    }
    let mut modes = args.clone();
    modes.extend([json!("--namespace"), json!("mode")]);
    json!({"mcpServers": {"harness": {"command": program.to_string_lossy(), "args": args}, "mode": {"command": program.to_string_lossy(), "args": modes}}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_speaks_mcp_and_its_tools_write_the_journal() {
        let dir = std::env::temp_dir().join(format!("mt-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let serving = Serving { data_dir: dir.clone(), root: dir.join("root").to_string_lossy().into_owned(), window: None, namespace: None, map_max_nodes: 200 };
        let messages = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "journal_add", "arguments": {"body": "written over mcp", "category": "finding", "tags": ["mcp"]}}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "journal_search", "arguments": {"query": "written over mcp"}}}),
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {"name": "no_such"}}),
            json!({"jsonrpc": "2.0", "id": 6, "method": "nonsense"}),
        ];
        let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
        let mut out = Vec::new();
        serve(&serving, input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 6, "a notification gets no answer");
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-06-18");
        let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"journal_add") && names.contains(&"mindmap_new") && names.contains(&"telegram_send"), "{names:?}");
        assert_eq!(replies[2]["result"]["isError"], false, "{}", replies[2]);
        assert!(replies[3]["result"]["content"][0]["text"].as_str().unwrap().contains("written over mcp"), "{}", replies[3]);
        assert_eq!(replies[4]["error"]["code"], -32602);
        assert_eq!(replies[5]["error"]["code"], -32601);
    }

    /// CON-114 over MCP: the server's own limit refuses the add past it, and the map keeps its nodes.
    #[test]
    fn the_mcp_server_refuses_a_node_past_its_limit() {
        let dir = std::env::temp_dir().join(format!("mt-mcp-limit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let serving = Serving { data_dir: dir.clone(), root: dir.join("root").to_string_lossy().into_owned(), window: None, namespace: None, map_max_nodes: 2 };
        let call = |id: u64, name: &str, arguments: Value| json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": arguments}});
        let messages = [
            call(1, "mindmap_new", json!({"title": "limited"})),
            call(2, "mindmap_add", json!({"map": 1, "text": "first", "kind": "note"})),
            call(3, "mindmap_add", json!({"map": 1, "text": "second", "kind": "note"})),
        ];
        let input: String = messages.iter().map(|m| format!("{m}\n")).collect();
        let mut out = Vec::new();
        serve(&serving, input.as_bytes(), &mut out).unwrap();
        let replies: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["result"]["isError"], false, "{}", replies[0]);
        assert_eq!(replies[1]["result"]["isError"], false, "under the limit a node passes: {}", replies[1]);
        assert_eq!(replies[2]["result"]["isError"], true, "{}", replies[2]);
        let text = replies[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("holds 2 nodes") && text.contains("limit is 2"), "{text}");
    }
}
