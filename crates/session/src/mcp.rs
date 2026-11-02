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

fn tools(serving: &Serving) -> Result<Vec<Rc<dyn Tool>>, String> {
    let client = Client::find(&serving.data_dir);
    let memory: Rc<dyn Memory> = match &client {
        Some(client) => Rc::new(harness_daemon::memory::DaemonMemory::new(client.clone(), serving.data_dir.clone(), &serving.root, "multitool")),
        None => Rc::new(crate::project_memory(&serving.data_dir.to_string_lossy(), &serving.root)?),
    };
    let mut out: Vec<Rc<dyn Tool>> = Vec::new();
    for name in harness_tools_memory::NAMES.into_iter().chain(harness_tools_memory::MORE) {
        if let Some(tool) = harness_tools_memory::tool(memory.clone(), name) {
            out.push(Rc::new(tool));
        }
    }
    out.push(Rc::new(crate::host_tools::TelegramSend { data_dir: serving.data_dir.clone(), from: serving.window.clone() }));
    if let (Some(client), Some((window, tab))) = (client, &serving.window) {
        out.push(Rc::new(Reload { client, window: window.clone(), tab: *tab }));
    }
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
        }),
        "ping" => json!({}),
        "tools/list" => {
            let list: Vec<Value> = tools.iter().map(|t| t.spec()).map(|s| json!({"name": s.name, "description": s.description, "inputSchema": s.schema})).collect();
            json!({"tools": list})
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            let Some(tool) = tools.iter().find(|t| t.spec().name == name) else {
                return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("no tool {name}")}}));
            };
            let input = if params["arguments"].is_object() { params["arguments"].clone() } else { json!({}) };
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
    json!({"mcpServers": {"harness": {"command": program.to_string_lossy(), "args": args}}}).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_speaks_mcp_and_its_tools_write_the_journal() {
        let dir = std::env::temp_dir().join(format!("mt-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("root")).unwrap();
        let serving = Serving { data_dir: dir.clone(), root: dir.join("root").to_string_lossy().into_owned(), window: None };
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
}
