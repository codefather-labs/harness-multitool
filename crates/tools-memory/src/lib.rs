//! The agent's tools over `Memory` (IF-4, IF-7). A mind map and a flowchart are separate tools
//! (ADR 0011). Each result is JSON: the model reads it, and the frontend draws it as a card.

use std::rc::Rc;

use harness_api::*;
use serde_json::{Value, json};

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

fn ids(v: &Value) -> Vec<u64> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

fn output(result: PluginResult<Value>) -> ToolOutput {
    match result {
        Ok(value) => ToolOutput::ok(value.to_string()),
        Err(e) => ToolOutput::error(e.to_string()),
    }
}

fn require<'a>(input: &'a Value, name: &str) -> PluginResult<&'a str> {
    input[name].as_str().filter(|s| !s.is_empty()).ok_or_else(|| PluginError::new(format!("{name} is required")))
}

fn number(input: &Value, name: &str) -> PluginResult<u64> {
    input[name].as_u64().ok_or_else(|| PluginError::new(format!("{name} must be a number")))
}

/// What a memory tool does with its input.
type Action = for<'a> fn(&'a dyn Memory, &'a Value) -> BoxFuture<'a, PluginResult<Value>>;

pub struct MemoryTool {
    memory: Rc<dyn Memory>,
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
    action: Action,
}

impl Tool for MemoryTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec { name: self.name.into(), description: self.description.into(), schema: (self.schema)() }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![Capability::Memory])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            if !cx.granted.contains(&Capability::Memory) {
                return ToolOutput::error("not granted: Memory");
            }
            output((self.action)(self.memory.as_ref(), input).await)
        })
    }
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

/// The names of all the tools, as the configuration selects them.
pub const NAMES: [&str; 10] = [
    "journal_add",
    "journal_search",
    "journal_get",
    "journal_close",
    "mindmap_new",
    "mindmap_add",
    "mindmap_show",
    "flowchart_new",
    "flowchart_add",
    "flowchart_show",
];

/// The rest of the store: a delegate gets them, since its project's rules use them. A model on its
/// own loop has them by name in the configuration; each costs prompt on a small one.
pub const MORE: [&str; 13] = ["journal_list", "journal_agenda", "journal_mark", "journal_reopen", "journal_abandon", "journal_branch", "journal_link", "journal_tags", "map_edit", "map_node_state", "map_link", "map_ref", "map_close"];

fn more<'a>(memory: &'a dyn Memory, op: PluginResult<MemoryOp>) -> BoxFuture<'a, PluginResult<Value>> {
    Box::pin(async move { memory.more(op?).await })
}

fn map_new<'a>(memory: &'a dyn Memory, input: &'a Value, kind: MapKind) -> BoxFuture<'a, PluginResult<Value>> {
    let start = require(input, "title").map(|title| memory.map_new(kind, title, input["direction"].as_str()));
    finish_map(start)
}

/// Waits out a map operation and gives its map as the tool's JSON.
fn finish_map<'a>(start: PluginResult<BoxFuture<'a, PluginResult<MapData>>>) -> BoxFuture<'a, PluginResult<Value>> {
    Box::pin(async move {
        let map = start?.await?;
        Ok(serde_json::to_value(map).expect("a map serialises"))
    })
}

fn map_add<'a>(memory: &'a dyn Memory, input: &'a Value) -> BoxFuture<'a, PluginResult<Value>> {
    let start = (|| {
        let node = NewMapNode {
            text: require(input, "text")?.to_string(),
            kind: input["kind"].as_str().unwrap_or_default().to_string(),
            parent: input["parent"].as_u64(),
            after: input["after"].as_u64(),
            label: input["label"].as_str().unwrap_or_default().to_string(),
            refs: ids(&input["refs"]),
        };
        Ok(memory.map_add(number(input, "map")?, node))
    })();
    finish_map(start)
}

fn map_show<'a>(memory: &'a dyn Memory, input: &'a Value) -> BoxFuture<'a, PluginResult<Value>> {
    Box::pin(async move {
        let id = number(input, "map")?;
        let map = memory.map_read(id).await?.ok_or_else(|| PluginError::new(format!("no map #{id}")))?;
        Ok(serde_json::to_value(map).expect("a map serialises"))
    })
}

/// Every memory tool, by name.
pub fn tool(memory: Rc<dyn Memory>, name: &str) -> Option<MemoryTool> {
    let tool = |name, description, schema: fn() -> Value, action: Action| Some(MemoryTool { memory: memory.clone(), name, description, schema, action });
    match name {
        "journal_add" => tool(
            "journal_add",
            "Record a decision, finding or update in the project's journal, one per entry, with the why. The category says what kind of entry it is. Flags are optional, any of: todo, question, problem, unverified, verified, specification, constraint, important, idea, concept, comment, deprecated.",
            || {
                object(
                    json!({
                        "body": {"type": "string"},
                        "category": {"type": "string", "description": "decision, finding, bug, lesson, update, ..."},
                        "tags": {"type": "array", "items": {"type": "string"}},
                        "flags": {"type": "array", "items": {"type": "string"}},
                        "salience": {"type": "string", "enum": ["high", "medium", "low"]},
                        "feature": {"type": "string"},
                    }),
                    &["body", "category", "tags"],
                )
            },
            |memory, input| {
                Box::pin(async move {
                    let entry = JournalEntry {
                        body: require(input, "body")?.to_string(),
                        category: require(input, "category")?.to_string(),
                        tags: strings(&input["tags"]),
                        flags: strings(&input["flags"]),
                        salience: input["salience"].as_str().map(str::to_string),
                        feature: input["feature"].as_str().map(str::to_string),
                    };
                    let record = memory.journal_add(entry).await?;
                    Ok(serde_json::to_value(record).expect("an entry serialises"))
                })
            },
        ),
        "journal_search" => tool(
            "journal_search",
            "Search the project's journal for earlier decisions and findings before deciding something.",
            || object(json!({"query": {"type": "string"}}), &["query"]),
            |memory, input| {
                Box::pin(async move {
                    let found = memory.journal_search(require(input, "query")?).await?;
                    Ok(json!({"entries": found}))
                })
            },
        ),
        "journal_get" => tool(
            "journal_get",
            "Read one journal entry in full.",
            || object(json!({"id": {"type": "integer"}}), &["id"]),
            |memory, input| {
                Box::pin(async move {
                    let id = number(input, "id")?;
                    let record = memory.journal_get(id).await?.ok_or_else(|| PluginError::new(format!("no entry #{id}")))?;
                    Ok(serde_json::to_value(record).expect("an entry serialises"))
                })
            },
        ),
        "journal_close" => tool(
            "journal_close",
            "Close an entry's open work items, saying how it was resolved.",
            || object(json!({"id": {"type": "integer"}, "resolution": {"type": "string"}}), &["id", "resolution"]),
            |memory, input| {
                Box::pin(async move {
                    let id = number(input, "id")?;
                    memory.journal_close(id, require(input, "resolution")?).await?;
                    Ok(json!({"closed": id}))
                })
            },
        ),
        "mindmap_new" => tool(
            "mindmap_new",
            "Start a mind map: the map of what is known about a topic. It keeps decisions, questions and constraints, and answers why the system is the way it is.",
            || object(json!({"title": {"type": "string"}}), &["title"]),
            |memory, input| map_new(memory, input, MapKind::MindMap),
        ),
        "mindmap_add" => tool(
            "mindmap_add",
            "Add a node to a mind map, under `parent` (the root by default). Kinds: fact, hypothesis, question, decision, task, idea, goal, dead-end, note. `refs` are journal entries.",
            || {
                object(
                    json!({"map": {"type": "integer"}, "text": {"type": "string"}, "kind": {"type": "string"}, "parent": {"type": "integer"}, "refs": {"type": "array", "items": {"type": "integer"}}}),
                    &["map", "text"],
                )
            },
            map_add,
        ),
        "mindmap_show" => tool("mindmap_show", "Read a mind map and draw it for the user.", || object(json!({"map": {"type": "integer"}}), &["map"]), map_show),
        "flowchart_new" => tool(
            "flowchart_new",
            "Start a flowchart: how something flows from a start to an end. `direction` is down or right.",
            || object(json!({"title": {"type": "string"}, "direction": {"type": "string", "enum": ["down", "right"]}}), &["title"]),
            |memory, input| map_new(memory, input, MapKind::Flowchart),
        ),
        "flowchart_add" => tool(
            "flowchart_add",
            "Add a symbol to a flowchart, with an arrow from `after`. Kinds: start, end, process, decision, io, subprocess, document, data, manual, prep, connector, delay, note, and, or, not, nand, nor, xor, xnor. `label` is the arrow's condition.",
            || {
                object(
                    json!({"map": {"type": "integer"}, "text": {"type": "string"}, "kind": {"type": "string"}, "after": {"type": "integer"}, "label": {"type": "string"}, "refs": {"type": "array", "items": {"type": "integer"}}}),
                    &["map", "text", "kind"],
                )
            },
            map_add,
        ),
        "flowchart_show" => tool("flowchart_show", "Read a flowchart and draw it for the user.", || object(json!({"map": {"type": "integer"}}), &["map"]), map_show),
        "journal_list" => tool(
            "journal_list",
            "List journal entries, newest first, by category, tags (any of), flags (any of), feature. `open` narrows to open or closed items. Reports the total; page with `offset`.",
            || object(json!({"category": {"type": "string"}, "tags": {"type": "array", "items": {"type": "string"}}, "flags": {"type": "array", "items": {"type": "string"}}, "open": {"type": "boolean"}, "feature": {"type": "string"}, "offset": {"type": "integer"}, "limit": {"type": "integer"}}), &[]),
            |memory, input| {
                more(memory, Ok(MemoryOp::JournalList {
                    category: input["category"].as_str().map(str::to_string),
                    tags: strings(&input["tags"]),
                    flags: strings(&input["flags"]),
                    open: input["open"].as_bool(),
                    feature: input["feature"].as_str().map(str::to_string),
                    offset: input["offset"].as_u64().unwrap_or(0) as usize,
                    limit: input["limit"].as_u64().unwrap_or(20) as usize,
                }))
            },
        ),
        "journal_agenda" => tool(
            "journal_agenda",
            "What is open in the journal, flag by flag: todos, questions, problems, and the specifications and constraints that bind the work. Read it on a fresh context.",
            || object(json!({"limit": {"type": "integer", "description": "entries per flag, 10 by default"}}), &[]),
            |memory, input| more(memory, Ok(MemoryOp::JournalAgenda { limit: input["limit"].as_u64().unwrap_or(10) as usize })),
        ),
        "journal_mark" => tool(
            "journal_mark",
            "Set flags on an existing entry, or with `remove` take marks off. For example deprecated when an entry stops being true. An unverified flag lifts only by an entry that verifies it.",
            || object(json!({"id": {"type": "integer"}, "flags": {"type": "array", "items": {"type": "string"}}, "remove": {"type": "boolean"}}), &["id", "flags"]),
            |memory, input| {
                more(memory, number(input, "id").map(|id| {
                    let flags = strings(&input["flags"]);
                    if input["remove"] == true { MemoryOp::JournalUnmark { id, flags } } else { MemoryOp::JournalMark { id, flags } }
                }))
            },
        ),
        "journal_reopen" => tool(
            "journal_reopen",
            "Reopen an entry's closed work items: the named flags, or all.",
            || object(json!({"id": {"type": "integer"}, "flags": {"type": "array", "items": {"type": "string"}}}), &["id"]),
            |memory, input| more(memory, number(input, "id").map(|id| MemoryOp::JournalReopen { id, flags: strings(&input["flags"]) })),
        ),
        "journal_abandon" => tool(
            "journal_abandon",
            "Close a todo whose work was dropped, not done: no evidence is needed, the resolution says why, and it is recorded as abandoned.",
            || object(json!({"id": {"type": "integer"}, "resolution": {"type": "string"}}), &["id", "resolution"]),
            |memory, input| more(memory, number(input, "id").and_then(|id| Ok(MemoryOp::JournalAbandon { id, resolution: require(input, "resolution")?.to_string() }))),
        ),
        "journal_branch" => tool(
            "journal_branch",
            "The work branch an entry belongs to: its todo, what implements and verifies it, whether it is open, verified or closed.",
            || object(json!({"id": {"type": "integer"}}), &["id"]),
            |memory, input| more(memory, number(input, "id").map(|id| MemoryOp::JournalBranch { id })),
        ),
        "journal_link" => tool(
            "journal_link",
            "Link two entries whose text does not mention each other. `kind` is cites (default), replaces or fixes.",
            || object(json!({"from": {"type": "integer"}, "to": {"type": "integer"}, "kind": {"type": "string", "enum": ["cites", "replaces", "fixes"]}}), &["from", "to"]),
            |memory, input| more(memory, number(input, "from").and_then(|from| Ok(MemoryOp::JournalLink { from, to: number(input, "to")?, kind: input["kind"].as_str().unwrap_or("cites").to_string() }))),
        ),
        "journal_tags" => tool(
            "journal_tags",
            "The journal's tag vocabulary, most used first, and its categories. Reuse these rather than near-synonyms.",
            || object(json!({"category": {"type": "string"}}), &[]),
            |memory, input| more(memory, Ok(MemoryOp::JournalTags { category: input["category"].as_str().map(str::to_string) })),
        ),
        "map_edit" => tool(
            "map_edit",
            "Change a node of a mind map or a flowchart: its text, its kind, or both.",
            || object(json!({"map": {"type": "integer"}, "node": {"type": "integer"}, "text": {"type": "string"}, "kind": {"type": "string"}}), &["map", "node"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapEdit { map, node: number(input, "node")?, text: input["text"].as_str().map(str::to_string), kind: input["kind"].as_str().map(str::to_string) }))),
        ),
        "map_node_state" => tool(
            "map_node_state",
            "Mark a node of a map: done, dropped, or open again.",
            || object(json!({"map": {"type": "integer"}, "node": {"type": "integer"}, "state": {"type": "string", "enum": ["open", "done", "dropped"]}}), &["map", "node", "state"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapNodeState { map, node: number(input, "node")?, state: require(input, "state")?.to_string() }))),
        ),
        "map_link" => tool(
            "map_link",
            "Draw an arrow between two nodes of a map, with a label such as contradicts, a condition, or a loop back. `refs` are journal entries.",
            || object(json!({"map": {"type": "integer"}, "from": {"type": "integer"}, "to": {"type": "integer"}, "label": {"type": "string"}, "refs": {"type": "array", "items": {"type": "integer"}}}), &["map", "from", "to"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapLink { map, from: number(input, "from")?, to: number(input, "to")?, label: input["label"].as_str().unwrap_or_default().to_string(), refs: ids(&input["refs"]) }))),
        ),
        "map_ref" => tool(
            "map_ref",
            "Point a node of a map at the journal entries it rests on.",
            || object(json!({"map": {"type": "integer"}, "node": {"type": "integer"}, "entries": {"type": "array", "items": {"type": "integer"}}}), &["map", "node", "entries"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapRef { map, node: number(input, "node")?, entries: ids(&input["entries"]) }))),
        ),
        "map_close" => tool(
            "map_close",
            "Close a map whose task is done. It stays readable.",
            || object(json!({"map": {"type": "integer"}}), &["map"]),
            |memory, input| more(memory, number(input, "map").map(|map| MemoryOp::MapClose { map })),
        ),
        _ => None,
    }
}
