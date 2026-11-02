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

/// Orchestration mode's tools (CON-100). The daemon holds the orchestra, so each is a word to it.
pub const MODE_TOOLS: [&str; 6] = ["mode__orchestrate", "mode__agent_models", "mode__agent_spawn", "mode__agent_read", "mode__agent_answer", "mode__agent_close"];

pub struct ModeTool {
    pub name: &'static str,
    pub client: harness_daemon::client::Client,
    pub window: String,
    pub tab: u64,
}

impl Tool for ModeTool {
    fn spec(&self) -> ToolSpec {
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
                "HARNESS MODE, orchestration only. Start an agent in a new tab of your group, on this authorized adapter and model, in this same project and git, with no history. It sends you word when it is ready; give it work then with agent_send. At most 8. Answers that its tab is opening.",
                json!({"type": "object", "properties": {"nick": {"type": "string", "description": "What it goes by; unique among the open."}, "adapter": {"type": "string"}, "model": {"type": "string"}}, "required": ["nick", "adapter", "model"]}),
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
