//! CON-20: Memory (IF-7) over the daemon, which owns the store. A session calls this; the
//! daemon runs the call on the project's journal. Calls block their session's thread, which is
//! the session's alone, as the store's own calls do.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use harness_api::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::Client;

/// How long a call waits for a daemon being replaced, as in a hot reload.
const SWAP_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub root: String,
    pub owner: String,
    pub call: Call,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Call {
    JournalAdd { entry: JournalEntry },
    JournalSearch { query: String },
    JournalGet { id: u64 },
    JournalClose { id: u64, resolution: String },
    JournalBinding { limit: usize },
    JournalList { filter: JournalFilter },
    MapList,
    MapNew { kind: MapKind, title: String, direction: Option<String> },
    MapAdd { map: u64, node: NewMapNode },
    MapRead { map: u64 },
}

#[derive(Debug)]
pub enum CallError {
    /// The call was not applied and another address may work: retrying cannot double it.
    Stale(String),
    Failed(String),
}

/// The daemon's side: one call on one project's store.
pub fn serve(store: &dyn Memory, call: Call) -> Result<Value, String> {
    fn answer<T: Serialize>(result: PluginResult<T>) -> Result<Value, String> {
        serde_json::to_value(result.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
    match call {
        Call::JournalAdd { entry } => answer(crate::now_ready(store.journal_add(entry))),
        Call::JournalSearch { query } => answer(crate::now_ready(store.journal_search(&query))),
        Call::JournalGet { id } => answer(crate::now_ready(store.journal_get(id))),
        Call::JournalClose { id, resolution } => answer(crate::now_ready(store.journal_close(id, &resolution))),
        Call::JournalBinding { limit } => answer(crate::now_ready(store.journal_binding(limit))),
        Call::JournalList { filter } => answer(crate::now_ready(store.journal_list(filter))),
        Call::MapList => answer(crate::now_ready(store.map_list())),
        Call::MapNew { kind, title, direction } => answer(crate::now_ready(store.map_new(kind, &title, direction.as_deref()))),
        Call::MapAdd { map, node } => answer(crate::now_ready(store.map_add(map, node))),
        Call::MapRead { map } => answer(crate::now_ready(store.map_read(map))),
    }
}

/// The session's side.
pub struct DaemonMemory {
    client: RefCell<Client>,
    data_dir: PathBuf,
    root: String,
    owner: String,
}

impl DaemonMemory {
    pub fn new(client: Client, data_dir: PathBuf, root: &str, owner: &str) -> DaemonMemory {
        DaemonMemory { client: RefCell::new(client), data_dir, root: root.to_string(), owner: owner.to_string() }
    }

    fn call<T: DeserializeOwned>(&self, call: Call) -> PluginResult<T> {
        let request = Request { root: self.root.clone(), owner: self.owner.clone(), call };
        let deadline = Instant::now() + SWAP_WAIT;
        loop {
            let client = self.client.borrow().clone();
            match client.memory(&request) {
                Ok(value) => return serde_json::from_value(value).map_err(|e| PluginError::new(format!("the daemon's answer: {e}"))),
                Err(CallError::Failed(message)) => return Err(PluginError::new(message)),
                Err(CallError::Stale(message)) => {
                    if Instant::now() >= deadline {
                        return Err(PluginError::new(format!("the memory daemon: {message}")));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    if let Some(found) = Client::find(&self.data_dir) {
                        *self.client.borrow_mut() = found;
                    }
                }
            }
        }
    }

    fn ready<T: DeserializeOwned + 'static>(&self, call: Call) -> BoxFuture<'_, PluginResult<T>> {
        let result = self.call(call);
        Box::pin(std::future::ready(result))
    }
}

impl Memory for DaemonMemory {
    fn journal_add(&self, entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
        self.ready(Call::JournalAdd { entry })
    }
    fn journal_search(&self, query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        self.ready(Call::JournalSearch { query: query.to_string() })
    }
    fn journal_get(&self, id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
        self.ready(Call::JournalGet { id })
    }
    fn journal_close(&self, id: u64, resolution: &str) -> BoxFuture<'_, PluginResult<()>> {
        self.ready(Call::JournalClose { id, resolution: resolution.to_string() })
    }
    fn journal_binding(&self, limit: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        self.ready(Call::JournalBinding { limit })
    }
    fn journal_list(&self, filter: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        self.ready(Call::JournalList { filter })
    }
    fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>> {
        self.ready(Call::MapList)
    }
    fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
        self.ready(Call::MapNew { kind, title: title.to_string(), direction: direction.map(str::to_string) })
    }
    fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
        self.ready(Call::MapAdd { map, node })
    }
    fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
        self.ready(Call::MapRead { map })
    }
}
