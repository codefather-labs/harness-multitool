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
