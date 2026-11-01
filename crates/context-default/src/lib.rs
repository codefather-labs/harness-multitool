//! The default context (IF-8): the whole conversation, with hook context and the project's root
//! in the system prompt. Journal- and map-aware compaction comes with M3.

use harness_api::*;

pub struct WholeConversation {
    pub root: String,
    /// One more line for the system prompt, e.g. where this window's callbacks go.
    pub note: Option<String>,
}

impl ContextStrategy for WholeConversation {
    fn build<'a>(&'a self, state: &'a SessionState, _memory: &'a dyn Memory) -> BoxFuture<'a, PluginResult<TurnContext>> {
        let mut system = format!("You are a coding agent working in the project at {}. Use the tools to read, change and check the code. Paths are relative to the project unless absolute.", self.root);
        for line in self.note.iter().chain(&state.hook_context) {
            system.push('\n');
            system.push_str(line);
        }
        Box::pin(std::future::ready(Ok(TurnContext { system: Some(system), messages: state.messages.clone() })))
    }
}

pub struct NoMemory;

fn none<'a, T: 'a>() -> BoxFuture<'a, PluginResult<T>> {
    Box::pin(std::future::ready(Err(PluginError::new("no memory backend is selected"))))
}

impl Memory for NoMemory {
    fn journal_add(&self, _entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
        none()
    }

    fn journal_search(&self, _query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn journal_get(&self, _id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(None)))
    }

    fn journal_close(&self, _id: u64, _resolution: &str) -> BoxFuture<'_, PluginResult<()>> {
        none()
    }

    fn map_new(&self, _kind: MapKind, _title: &str, _direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
        none()
    }

    fn map_add(&self, _map: u64, _node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
        none()
    }

    fn map_read(&self, _map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}
