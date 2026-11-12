//! Tools the running binary gives its agent (ADR 0015): see what is plugged in, and ask for a
//! hot reload, which the frontend carries out at a safe point.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use harness_api::*;
use serde_json::{Value, json};

pub struct PluginsList(pub Arc<dyn Fn() -> Value + Send + Sync>);

impl Tool for PluginsList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "plugins_list".into(),
            description: "Answer with the plugins and callback handlers connected in this build, and what this session has chosen of them. Use it to see what the harness can do here before assuming.".into(),
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, _input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(std::future::ready(ToolOutput::ok((self.0)().to_string())))
    }
}

pub struct PluginsReload(pub Arc<AtomicBool>);

impl Tool for PluginsReload {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "plugins_reload".into(),
            description: "Rebuild this harness from its source and swap the running binary without ending the conversation. It builds now and swaps when your turn ends; then you get a message saying so, or why the build failed, and you go on. Finish background work first: it ends with the swap.".into(),
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Command("rebuild and restart the harness".into())])
    }

    fn execute<'a>(&'a self, _input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let need = Capability::Command("rebuild and restart the harness".into());
        let output = if cx.granted.contains(&need) {
            self.0.store(true, Ordering::SeqCst);
            ToolOutput::ok("The reload will run when this turn ends.")
        } else {
            ToolOutput::error("not granted")
        };
        Box::pin(std::future::ready(output))
    }
}

/// The agent's reply through the Telegram channel, which the daemon holds. It leaves this
/// machine, so it asks permission like any outward action.
pub struct TelegramSend {
    pub data_dir: std::path::PathBuf,
    /// The window and tab asking: it answers where that session is bound.
    pub from: Option<(String, u64)>,
}

impl Tool for TelegramSend {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "telegram_send".into(),
            description: "Send a message to the operator on Telegram, to the chat or topic bound to this conversation or else the one they last wrote from. They cannot see your normal answer there, so use it to answer a message that came from Telegram. Asks permission like any outward action. Answers how many messages it took.".into(),
            schema: json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Command("send a Telegram message".into())])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match (input["text"].as_str(), harness_daemon::client::Client::find(&self.data_dir)) {
            (None, _) => ToolOutput::error("text is required".to_string()),
            (_, None) => ToolOutput::error("the daemon is not running".to_string()),
            (Some(text), Some(client)) => match client.telegram_send(text, self.from.as_ref().map(|f| (f.0.as_str(), f.1))) {
                Ok(parts) => ToolOutput::ok(format!("sent, in {parts} message(s)")),
                Err(error) => ToolOutput::error(error),
            },
        };
        Box::pin(std::future::ready(answer))
    }
}

/// The contract of a debug router (CON-27).
pub struct DebugContract;

impl Tool for DebugContract {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "debug_contract".into(),
            description: "Answer with the OpenAPI contract every step-debug router is built by. A utility to read, not a switch: debug mode itself is mode__debug. Read it before building a router for a pipeline.".into(),
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, _input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(std::future::ready(ToolOutput::ok(harness_daemon::stepdebug::CONTRACT.to_string())))
    }
}

/// The project's saved step-debug runs.
pub struct DebugRuns {
    pub root: String,
}

impl Tool for DebugRuns {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "debug_runs".into(),
            description: "Answer with the latest saved step-debug runs of this project, newest first, with their steps, to compare a run with an earlier one. A utility to read, not a switch: debug mode itself is mode__debug.".into(),
            schema: json!({"type": "object", "properties": {"limit": {"type": "integer"}}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::ReadPath(".harness/debug/runs.sqlite".into())])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let limit = input["limit"].as_u64().unwrap_or(5) as usize;
        let answer = match harness_daemon::stepdebug::runs(&self.root, limit) {
            Ok(runs) => ToolOutput::ok(serde_json::to_string(&runs).unwrap_or_default()),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}

/// The conversations on this machine (CON-99).
pub struct AgentList {
    pub client: harness_daemon::client::Client,
    pub window: String,
}

impl Tool for AgentList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "agent_list".into(),
            description: "List the other agents on this machine: each open conversation's nick, id, adapter, model, folder and whether it is running. Use it before agent_send, to see who is there.".into(),
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, _input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match self.client.conversation_op(&json!({"window": self.window, "op": "agent_list"})) {
            Ok(reply) => ToolOutput::ok(reply.to_string()),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}

/// A message to another agent (CON-99).
pub struct AgentSend {
    pub client: harness_daemon::client::Client,
    pub window: String,
    pub tab: u64,
}

impl Tool for AgentSend {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "agent_send".into(),
            description: "Send a message to another agent on this machine, by its nick or id. It reaches them marked as data from outside, with your name, and starts or queues a run of theirs. They answer the same way, so end your turn rather than waiting. Limited to 60 in 10 minutes. Answers whether it was delivered or queued.".into(),
            schema: json!({"type": "object", "properties": {"to": {"type": "string", "description": "A nick, any case, or an id from agent_list."}, "text": {"type": "string"}}, "required": ["to", "text"]}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match self.client.conversation_op(&json!({"window": self.window, "op": "agent_send", "tab": self.tab, "to": input["to"], "text": input["text"]})) {
            Ok(reply) => ToolOutput::ok(reply.to_string()),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}

/// The game tools (ADR 0029): each a word to the daemon, which holds the tables.
pub const GAME_TOOLS: [&str; 7] = ["game_tables", "game_join", "game_decline", "game_state", "game_history", "game_act", "game_leave"];

pub struct GameTool {
    pub name: &'static str,
    pub client: harness_daemon::client::Client,
    pub window: String,
    pub tab: u64,
}

impl Tool for GameTool {
    fn spec(&self) -> ToolSpec {
        let table = json!({"table": {"type": "string", "description": "The table's id, such as T1."}});
        let (description, schema) = match self.name {
            "game_tables" => ("The game tables you are invited to or sit at, each with its state and what everyone can see.", json!({"type": "object", "properties": {}})),
            "game_join" => ("Join a table you were invited to. The game starts when everyone has answered.", json!({"type": "object", "properties": table, "required": ["table"]})),
            "game_decline" => ("Say no to a table you were invited to.", json!({"type": "object", "properties": table, "required": ["table"]})),
            "game_state" => ("The hand being played now, as you may see it: its street, the board, the pot and the side pots, the button and the blinds, who is to act, how many are still in the hand and in the game, each player's stack and what they have put in this round and this hand; under `you` your cards, your stack, what you have put in, what it is to call, what you may do, the least raise-to and the most you can put in.", json!({"type": "object", "properties": table, "required": ["table"]})),
            "game_history" => ("What has happened in the hand being played now, in order from the deal: the blinds, every action with its player and amount, every street with its board, and at the end the winners and the cards shown. Your own cards are in it, no one else's. Only the current hand.", json!({"type": "object", "properties": table, "required": ["table"]})),
            "game_act" => (
                "Act at a table when it is your turn: fold, check, call, bet or raise (`amount` is the total you bet in this round), or all_in. A move that is not legal is refused, saying why.",
                json!({"type": "object", "properties": {"table": {"type": "string"}, "do": {"type": "string", "enum": ["fold", "check", "call", "bet", "raise", "all_in"]}, "amount": {"type": "integer"}}, "required": ["table", "do"]}),
            ),
            _ => ("Leave a table: you fold now and at every turn after.", json!({"type": "object", "properties": table, "required": ["table"]})),
        };
        ToolSpec { name: self.name.into(), description: description.into(), schema }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let mut op = input.clone();
        if !op.is_object() {
            op = json!({});
        }
        op["window"] = json!(self.window);
        op["op"] = json!(self.name);
        op["tab"] = json!(self.tab);
        let answer = match self.client.conversation_op(&op) {
            Ok(reply) => ToolOutput::ok(reply.to_string()),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}

/// Orchestration mode's tools (CON-100). The daemon holds the orchestra, so each is a word to it.
pub const MODE_TOOLS: [&str; 6] = ["mode__orchestrate", "mode__agent_models", "mode__agent_spawn", "mode__agent_read", "mode__agent_answer", "mode__agent_close"];

/// The plan and debug modes over MCP: on a native loop the engine answers them itself.
pub const DELEGATE_MODE_TOOLS: [&str; 3] = ["mode__plan_enter", "mode__plan_exit", "mode__debug"];

/// How the program of a slave is told to reach [`PermissionAsk`], over the `mode` server.
pub const PROMPT_TOOL: &str = "mcp__mode__permission_ask";

/// How long a slave's program waits on its master before a request is denied (CON-108).
const PERMISSION_WAIT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// A slave's program asks here what it would ask a person; its master answers (CON-108). The call
/// waits for that, which is what the program wants of a permission prompt tool.
pub struct PermissionAsk {
    pub client: harness_daemon::client::Client,
    pub window: String,
    pub tab: u64,
}

impl Tool for PermissionAsk {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mode__permission_ask".into(),
            description: "HARNESS MODE, for the program that runs you, not for you to call: it puts a permission request to your orchestrator and waits for the answer. Ask for what you need by doing it.".into(),
            schema: json!({"type": "object", "properties": {"tool_name": {"type": "string"}, "input": {"type": "object"}, "tool_use_id": {"type": "string"}}, "required": ["tool_name", "input"]}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let verdict = wait_for_master(self, input);
        let reply = match verdict {
            Ok(()) => json!({"behavior": "allow", "updatedInput": input["input"]}),
            Err(why) => json!({"behavior": "deny", "message": why}),
        };
        Box::pin(std::future::ready(ToolOutput::ok(reply.to_string())))
    }
}

/// `Err` says why it is denied.
fn wait_for_master(ask: &PermissionAsk, input: &Value) -> Result<(), String> {
    let op = |name: &str, more: Value| {
        let mut op = json!({"window": ask.window, "op": name, "tab": ask.tab});
        op.as_object_mut().expect("an object").extend(more.as_object().cloned().unwrap_or_default());
        ask.client.conversation_op(&op)
    };
    let asked = op("slave_ask", json!({"tool": input["tool_name"], "input": input["input"]}))?;
    if let Some(why) = asked["refused"].as_str() {
        return Err(why.to_string());
    }
    if asked["answer"].is_string() {
        return Ok(());
    }
    let id = asked["id"].clone();
    let whose = if asked["user"] == true { "The user" } else { "Your orchestrator" };
    let end = std::time::Instant::now() + PERMISSION_WAIT;
    while std::time::Instant::now() < end {
        let state = op("slave_asked", json!({"id": id}))?;
        match state["answer"].as_str() {
            Some("deny") if whose == "The user" => return Err("The user did not allow this. Do not try it another way; say why you need it if your work does.".to_string()),
            Some("deny") => return Err("Your orchestrator did not allow this. Do not try it another way; tell it with agent_send if your work needs it.".to_string()),
            Some(_) => return Ok(()),
            None => std::thread::sleep(std::time::Duration::from_millis(200)),
        }
    }
    Err(format!("{whose} did not answer in time, so this is denied."))
}

pub struct ModeTool {
    pub name: &'static str,
    pub client: harness_daemon::client::Client,
    pub window: String,
    pub tab: u64,
}

impl Tool for ModeTool {
    fn spec(&self) -> ToolSpec {
        if let Some(own) = harness_core::engine::own_tool_specs().into_iter().find(|spec| spec.name == self.name) {
            return own;
        }
        let (description, schema) = match self.name {
            "mode__orchestrate" => (
                "HARNESS MODE, not a utility: on or off. Enter orchestration mode to start other agents and give them work: you become their master, your prompt gains the mode's rules, and the tools to start, read and close agents appear. Answers with those rules. Your agents stay when you leave the mode.",
                json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]}),
            ),
            "mode__agent_models" => (
                "HARNESS MODE, orchestration only. List the adapters you can start an agent on, those authorized here, with the models known for each. Choose from this for mode__agent_spawn.",
                json!({"type": "object", "properties": {}}),
            ),
            "mode__agent_spawn" => (
                "HARNESS MODE, orchestration only. Start an agent in a new tab of your group, on this authorized adapter and model, in this same project and git, with no history, on the open journal entry `task`, which it starts on at once. It asks you to close the entry when it is done, and then to close its session. At most 8. Answers that its tab is opening.",
                json!({"type": "object", "properties": {"nick": {"type": "string", "description": "What it goes by; unique among the open."}, "adapter": {"type": "string"}, "model": {"type": "string"}, "task": {"type": "integer", "description": "The id of the open journal entry that is its work. Write it first with journal_add."}}, "required": ["nick", "adapter", "model", "task"]}),
            ),
            "mode__agent_answer" => (
                "HARNESS MODE, orchestration only. Answer one of your agents' permission requests: once, always (for this tool and target in that session) or deny. The user has left these to you, so it runs on the user's machine on your word: read what it asks, and allow only what its work needs. Answers that it was answered.",
                json!({"type": "object", "properties": {"nick": {"type": "string"}, "answer": {"type": "string", "enum": ["once", "always", "deny"]}}, "required": ["nick", "answer"]}),
            ),
            "mode__agent_read" => (
                "HARNESS MODE, orchestration only. Read what one of your agents has done: each message it got, each call it made in order with its arguments, and its last words. Use it to check its work against the agent workflow specification.",
                json!({"type": "object", "properties": {"nick": {"type": "string"}}, "required": ["nick"]}),
            ),
            _ => (
                "HARNESS MODE, orchestration only. End one of your agents: its process stops and its tab closes. What it wrote to the journal and the project stays. Answers that it was closed.",
                json!({"type": "object", "properties": {"nick": {"type": "string"}}, "required": ["nick"]}),
            ),
        };
        ToolSpec { name: self.name.into(), description: description.into(), schema }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let mut op = input.clone();
        if !op.is_object() {
            op = json!({});
        }
        op["window"] = json!(self.window);
        op["tab"] = json!(self.tab);
        op["op"] = json!(self.name);
        let answer = match self.client.conversation_op(&op) {
            Ok(reply) => ToolOutput::ok(reply["text"].as_str().map_or_else(|| reply.to_string(), str::to_string)),
            Err(error) => ToolOutput::error(error),
        };
        Box::pin(std::future::ready(answer))
    }
}
