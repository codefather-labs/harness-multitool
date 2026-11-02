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
            description: "List the plugins and callback handlers connected in this build.".into(),
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
            description: "Rebuild this harness with its plugins and swap the running binary without ending the session. It happens once your turn is over.".into(),
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
            description: "Send a message to the operator on Telegram: to the chat they last wrote from. Use it to answer a message that came from Telegram.".into(),
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
            description: "The OpenAPI contract every step-debug router is built by.".into(),
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
            description: "The latest saved step-debug runs of this project, newest first, with their steps.".into(),
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
