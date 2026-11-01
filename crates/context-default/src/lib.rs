//! The default context (IF-8): the whole conversation, with hook context and the project's root
//! in the system prompt. Journal- and map-aware compaction arrives with the daemon (M3), and
//! until then there is a `Memory` that says so.

use harness_api::*;

pub struct WholeConversation {
    pub root: String,
}

impl ContextStrategy for WholeConversation {
    fn build<'a>(&'a self, state: &'a SessionState, _memory: &'a dyn Memory) -> BoxFuture<'a, PluginResult<TurnContext>> {
        let mut system = format!("You are a coding agent working in the project at {}. Use the tools to read, change and check the code. Paths are relative to the project unless absolute.", self.root);
        for line in &state.hook_context {
            system.push('\n');
            system.push_str(line);
        }
        Box::pin(std::future::ready(Ok(TurnContext { system: Some(system), messages: state.messages.clone() })))
    }
}

pub struct NoMemory;

impl Memory for NoMemory {
    fn journal_add(&self, _entry: JournalEntry) -> BoxFuture<'_, PluginResult<u64>> {
        Box::pin(std::future::ready(Err(PluginError::new("no memory backend is connected yet"))))
    }

    fn journal_search(&self, _query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalEntry>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }
}
