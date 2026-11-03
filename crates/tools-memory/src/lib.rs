//! The agent's tools over `Memory` (IF-4, IF-7). A mind map and a flowchart are separate tools
//! (ADR 0011). Each result is JSON: the model reads it, and the frontend draws it as a card.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use harness_api::*;
use serde_json::{Value, json};

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Each path in `proof_file` must name something: relative ones against the project's root, `~/`
/// against the home folder. Nothing else is looked at: `proof` is words (journal#607).
fn check_proofs(input: &Value, root: &str) -> PluginResult<()> {
    for file in strings(&input["proof_file"]) {
        let file = file.trim();
        let path = match file.strip_prefix("~/") {
            Some(rest) => std::env::var_os("HOME").map_or_else(|| PathBuf::from(file), |home| PathBuf::from(home).join(rest)),
            None if Path::new(file).is_absolute() => PathBuf::from(file),
            None => Path::new(root).join(file),
        };
        if !path.exists() {
            return Err(PluginError::new(format!("proof_file `{file}` names no file: nothing at {}", path.display())));
        }
    }
    Ok(())
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
    /// The most nodes a map may hold before a node is refused: the `map_max_nodes` setting, 0 for none (CON-114).
    map_max_nodes: u64,
}

impl MemoryTool {
    /// Refuses a node past the limit, counting the map's nodes root included (CON-114). Nothing is
    /// added when it refuses. A map that cannot be read is left to the add, which says what is wrong.
    async fn check_node_limit(&self, input: &Value) -> PluginResult<()> {
        if self.map_max_nodes == 0 {
            return Ok(());
        }
        let id = number(input, "map")?;
        let Some(map) = self.memory.map_read(id).await? else { return Ok(()) };
        let held = map.nodes.len() as u64;
        if held < self.map_max_nodes {
            return Ok(());
        }
        Err(PluginError::new(format!(
            "map #{id} holds {held} nodes, and the limit is {} (map_max_nodes): nothing was added. The limit is the user's to change in the settings, Memory.",
            self.map_max_nodes
        )))
    }
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
            // A proof named as a file must be there before the entry is stored (journal#607).
            if self.name == "journal_add" && let Err(e) = check_proofs(input, &cx.root) {
                return ToolOutput::error(e.to_string());
            }
            if matches!(self.name, "mindmap_add" | "flowchart_add") && let Err(e) = self.check_node_limit(input).await {
                return ToolOutput::error(e.to_string());
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
/// When an agent is to use `blocked_by`, where it decides (CON-91).
const BLOCKED_BY_RULE: &str = "The id of the journal entry that blocks this one. When a problem or a task cannot be solved now because another one in the journal has to be solved first, set this to that entry's id, writing the other one down first if it is not in the journal yet, and say in the body why it blocks. This entry then cannot close until that one is closed. Work on the blocker first. Do not set it for what is only related or merely easier afterwards: that is a citation.";

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

/// Every memory tool, by name. `map_max_nodes` is the most nodes a map may hold, 0 for none (CON-114).
pub fn tool(memory: Rc<dyn Memory>, name: &str, map_max_nodes: u64) -> Option<MemoryTool> {
    let tool = |name, description, schema: fn() -> Value, action: Action| Some(MemoryTool { memory: memory.clone(), name, description, schema, action, map_max_nodes });
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
                        "blocked_by": {"type": "integer", "description": BLOCKED_BY_RULE},
                        "proof": {"type": "array", "items": {"type": "string"}, "description": "What shows it was checked, in words: a test's name, a command and its result, a commit, a URL. A todo closes only on a verified entry with this or proof_file."}, "proof_file": {"type": "array", "items": {"type": "string"}, "description": "Only if the proof is a file: its path, relative to the project, absolute or under ~/. Each must exist, or the entry is refused. Leave it out when no file is needed."},
                    }),
                    &["body", "category", "tags"],
                )
            },
            |memory, input| {
                Box::pin(async move {
                    let category = require(input, "category")?.to_string();
                    let mut flags = strings(&input["flags"]);
                    // An entry of these kinds is open work: without its flag nothing would show it.
                    if ["todo", "question", "problem"].contains(&category.as_str()) && !flags.contains(&category) {
                        flags.push(category.clone());
                    }
                    let entry = JournalEntry {
                        body: require(input, "body")?.to_string(),
                        category,
                        tags: strings(&input["tags"]),
                        flags,
                        salience: input["salience"].as_str().map(str::to_string),
                        feature: input["feature"].as_str().map(str::to_string),
                        blocked_by: input["blocked_by"].as_u64(),
                        proofs: strings(&input["proof"]).into_iter().chain(strings(&input["proof_file"])).collect(),
                    };
                    let record = memory.journal_add(entry).await?;
                    Ok(serde_json::to_value(record).expect("an entry serialises"))
                })
            },
        ),
        "journal_search" => tool(
            "journal_search",
            "Search the project's journal by words for earlier decisions and findings, before deciding something or redoing work. Answers with the closest entries whole. Trust a replaced or deprecated entry only as history.",
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
            "Read one journal entry in full, with its flags, tags and what blocks it, by id. Use it on what journal_agenda or journal_list show only the start of, and before relying on an entry.",
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
            "Close an entry's open work items, saying how it was resolved. The answer's `released` names the entries this set free: they can be worked on now.",
            || object(json!({"id": {"type": "integer"}, "resolution": {"type": "string"}}), &["id", "resolution"]),
            |memory, input| {
                Box::pin(async move {
                    let id = number(input, "id")?;
                    memory.journal_close(id, require(input, "resolution")?).await?;
                    // The close stands whatever this says: it only tells more.
                    let released = memory.more(MemoryOp::JournalReleased { id }).await.ok().map_or(Value::Null, |answer| answer["released"].clone());
                    if released.as_array().is_some_and(|list| !list.is_empty()) {
                        Ok(json!({"closed": id, "released": released}))
                    } else {
                        Ok(json!({"closed": id}))
                    }
                })
            },
        ),
        "mindmap_new" => tool(
            "mindmap_new",
            "Start a mind map: the map of what is known about a topic. It keeps decisions, questions and constraints, and answers why the system is the way it is. Use it for a long task or a hunt for a cause. Answers with the map and its root node's id.",
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
        "mindmap_show" => tool("mindmap_show", "Read a mind map back as an outline, and draw it for the user. Use it after a resume or a compaction, before going on with a task or changing the map. Answers with its nodes and their states.", || object(json!({"map": {"type": "integer"}}), &["map"]), map_show),
        "flowchart_new" => tool(
            "flowchart_new",
            "Start a flowchart: how something flows from a start to an end, for a process, a request path or a pipeline. `direction` is down or right. Answers with the chart and its id; flowchart_add draws in it.",
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
        "flowchart_show" => tool("flowchart_show", "Read a flowchart back as an outline, and draw it for the user. Use it before changing the chart or walking the flow in debug mode. Answers with its symbols, arrows and states.", || object(json!({"map": {"type": "integer"}}), &["map"]), map_show),
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
            "What is open in the journal, flag by flag: todos, questions, problems, and the specifications and constraints that bind the work, each entry as its start: journal_get reads one whole. Read it on a fresh context. Those groups hold what can be worked on now; `blocked` is what waits for another entry, and `blocker_dropped` is what waited for something dropped instead of solved: re-read those, then lift the link with journal_mark.",
            || object(json!({"limit": {"type": "integer", "description": "entries per flag, 10 by default"}}), &[]),
            |memory, input| more(memory, Ok(MemoryOp::JournalAgenda { limit: input["limit"].as_u64().unwrap_or(10) as usize })),
        ),
        "journal_mark" => tool(
            "journal_mark",
            "Set flags on an existing entry, or with `remove` take marks off. For example deprecated when an entry stops being true. An unverified flag lifts only by an entry that verifies it. `blocked_by` instead of flags blocks the entry, or with `remove` lifts that block.",
            || object(json!({"id": {"type": "integer"}, "flags": {"type": "array", "items": {"type": "string"}}, "remove": {"type": "boolean"}, "blocked_by": {"type": "integer", "description": BLOCKED_BY_RULE}}), &["id"]),
            |memory, input| {
                more(memory, number(input, "id").and_then(|id| {
                    let flags = strings(&input["flags"]);
                    let remove = input["remove"] == true;
                    match input["blocked_by"].as_u64() {
                        Some(_) if !flags.is_empty() => Err(PluginError::new("flags or blocked_by, one at a time")),
                        Some(by) if remove => Ok(MemoryOp::JournalUnblock { id, by }),
                        Some(by) => Ok(MemoryOp::JournalBlock { id, by }),
                        None if flags.is_empty() => Err(PluginError::new("flags or blocked_by is required")),
                        None if remove => Ok(MemoryOp::JournalUnmark { id, flags }),
                        None => Ok(MemoryOp::JournalMark { id, flags }),
                    }
                }))
            },
        ),
        "journal_reopen" => tool(
            "journal_reopen",
            "Reopen an entry's closed work items, the named flags or all, when what was closed comes back. A reopened blocker blocks again whatever is still waiting for it. Answers with what was reopened.",
            || object(json!({"id": {"type": "integer"}, "flags": {"type": "array", "items": {"type": "string"}}}), &["id"]),
            |memory, input| more(memory, number(input, "id").map(|id| MemoryOp::JournalReopen { id, flags: strings(&input["flags"]) })),
        ),
        "journal_abandon" => tool(
            "journal_abandon",
            "Close a todo whose work was dropped, not done: no evidence is needed, the resolution says why, and it is recorded as abandoned. The answer's `released` names what waited for it: what they needed is still not there, so re-read each.",
            || object(json!({"id": {"type": "integer"}, "resolution": {"type": "string"}}), &["id", "resolution"]),
            |memory, input| more(memory, number(input, "id").and_then(|id| Ok(MemoryOp::JournalAbandon { id, resolution: require(input, "resolution")?.to_string() }))),
        ),
        "journal_branch" => tool(
            "journal_branch",
            "Show the work branch an entry belongs to: its todo, what implements and verifies it, and whether it is open, verified or closed. Use it to see what a todo still lacks before closing it.",
            || object(json!({"id": {"type": "integer"}}), &["id"]),
            |memory, input| more(memory, number(input, "id").map(|id| MemoryOp::JournalBranch { id })),
        ),
        "journal_link" => tool(
            "journal_link",
            "Link two entries whose text does not mention each other. `kind` is cites (default), replaces or fixes. A journal#N in an entry's text already links it. To say one waits for another, use blocked_by instead.",
            || object(json!({"from": {"type": "integer"}, "to": {"type": "integer"}, "kind": {"type": "string", "enum": ["cites", "replaces", "fixes"]}}), &["from", "to"]),
            |memory, input| more(memory, number(input, "from").and_then(|from| Ok(MemoryOp::JournalLink { from, to: number(input, "to")?, kind: input["kind"].as_str().unwrap_or("cites").to_string() }))),
        ),
        "journal_tags" => tool(
            "journal_tags",
            "Answer with the journal's tag vocabulary, most used first, and its categories. Read it before tagging an entry, and reuse these rather than near-synonyms: tags are how an entry is found again.",
            || object(json!({"category": {"type": "string"}}), &[]),
            |memory, input| more(memory, Ok(MemoryOp::JournalTags { category: input["category"].as_str().map(str::to_string) })),
        ),
        "map_edit" => tool(
            "map_edit",
            "Change a node of a mind map or a flowchart: its text, its kind, or both, by map and node id. Keep a map true as what you know changes. Answers with the map as it now stands.",
            || object(json!({"map": {"type": "integer"}, "node": {"type": "integer"}, "text": {"type": "string"}, "kind": {"type": "string"}}), &["map", "node"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapEdit { map, node: number(input, "node")?, text: input["text"].as_str().map(str::to_string), kind: input["kind"].as_str().map(str::to_string) }))),
        ),
        "map_node_state" => tool(
            "map_node_state",
            "Mark a node of a map as done, dropped, or open again, as work moves on. A dropped node stays visible, so what was ruled out is not tried twice. Answers with the map as it now stands.",
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
            "Point a node of a map at the journal entries it rests on, so the map holds where you are and the journal the detail. Answers with the map as it now stands.",
            || object(json!({"map": {"type": "integer"}, "node": {"type": "integer"}, "entries": {"type": "array", "items": {"type": "integer"}}}), &["map", "node", "entries"]),
            |memory, input| more(memory, number(input, "map").and_then(|map| Ok(MemoryOp::MapRef { map, node: number(input, "node")?, entries: ids(&input["entries"]) }))),
        ),
        "map_close" => tool(
            "map_close",
            "Close a map whose task is done, so it stops showing as open work. It stays readable by mindmap_show and flowchart_show. Answers with the map.",
            || object(json!({"map": {"type": "integer"}}), &["map"]),
            |memory, input| more(memory, number(input, "map").map(|map| MemoryOp::MapClose { map })),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder for one test, under the system's temporary folder.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tools-memory-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_files(files: &[&str]) -> Value {
        json!({"flags": ["verified"], "proof": ["any words"], "proof_file": files})
    }

    #[test]
    fn a_missing_proof_file_is_refused_naming_it() {
        let root = scratch("missing-relative");
        let err = check_proofs(&with_files(&["missing/out.txt"]), root.to_str().unwrap()).unwrap_err();
        assert!(err.to_string().contains("missing/out.txt"), "{err}");
        let absolute = root.join("gone.png");
        let err = check_proofs(&with_files(&[absolute.to_str().unwrap()]), "/nonexistent-root").unwrap_err();
        assert!(err.to_string().contains("gone.png"), "{err}");
    }

    #[test]
    fn an_existing_proof_file_is_accepted_relative_or_absolute() {
        let root = scratch("existing");
        std::fs::create_dir_all(root.join("proofs")).unwrap();
        std::fs::write(root.join("proofs/out.txt"), "ok").unwrap();
        assert!(check_proofs(&with_files(&["proofs/out.txt"]), root.to_str().unwrap()).is_ok());
        let file = root.join("proofs/out.txt");
        assert!(check_proofs(&with_files(&[file.to_str().unwrap()]), "/nonexistent-root").is_ok());
    }

    /// The owner's rule: a proof with no file needs no path, and its words are not read as one.
    #[test]
    fn words_in_proof_are_never_taken_for_a_path() {
        for words in ["cargo test -p x, crates/foo passes", "crates/a.rs::the_test", "crates/a.rs:120", "missing/out.txt", "https://example.com/x", "v1.2"] {
            assert!(check_proofs(&json!({"flags": ["verified"], "proof": [words]}), "/nonexistent-root").is_ok(), "{words}");
        }
        assert!(check_proofs(&json!({"flags": ["verified"]}), "/nonexistent-root").is_ok(), "no file, no path");
    }

    #[test]
    fn any_entry_with_a_proof_file_is_checked() {
        assert!(check_proofs(&json!({"flags": ["unverified"], "proof_file": ["missing/out.txt"]}), "/nonexistent-root").is_err());
    }

    /// Runs a future the in-memory double answers at once.
    fn now<T>(future: BoxFuture<'_, PluginResult<T>>) -> PluginResult<T> {
        let mut future = future;
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("the double answers at once"),
        }
    }

    /// Maps in memory: the three the limit needs. The journal is not used by these tests.
    #[derive(Default)]
    struct MapsOnly {
        maps: std::cell::RefCell<Vec<MapData>>,
    }

    fn unused<'a, T: 'a>() -> BoxFuture<'a, PluginResult<T>> {
        Box::pin(std::future::ready(Err(PluginError::new("not used by these tests"))))
    }

    impl Memory for MapsOnly {
        fn journal_add(&self, _: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
            unused()
        }
        fn journal_search(&self, _: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
            unused()
        }
        fn journal_get(&self, _: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
            unused()
        }
        fn journal_close(&self, _: u64, _: &str) -> BoxFuture<'_, PluginResult<()>> {
            unused()
        }
        fn journal_binding(&self, _: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
            unused()
        }
        fn journal_list(&self, _: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
            unused()
        }
        fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>> {
            unused()
        }
        fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
            let root = MapNode { id: 1, parent: None, kind: "topic".into(), text: title.into(), state: "open".into(), refs: vec![] };
            let map = MapData { id: self.maps.borrow().len() as u64 + 1, title: title.into(), kind, direction: direction.unwrap_or("right").into(), root: 1, nodes: vec![root], edges: vec![] };
            self.maps.borrow_mut().push(map.clone());
            Box::pin(std::future::ready(Ok(map)))
        }
        fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
            let mut maps = self.maps.borrow_mut();
            let found = maps.iter_mut().find(|m| m.id == map).map(|m| {
                let id = m.nodes.len() as u64 + 1;
                m.nodes.push(MapNode { id, parent: node.parent.or(Some(m.root)), kind: node.kind.clone(), text: node.text.clone(), state: "open".into(), refs: node.refs.clone() });
                m.clone()
            });
            Box::pin(std::future::ready(found.ok_or_else(|| PluginError::new(format!("no map #{map}")))))
        }
        fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
            Box::pin(std::future::ready(Ok(self.maps.borrow().iter().find(|m| m.id == map).cloned())))
        }
    }

    fn memory_with_map() -> Rc<MapsOnly> {
        let memory = Rc::new(MapsOnly::default());
        now(memory.map_new(MapKind::MindMap, "t", None)).unwrap();
        memory
    }

    fn add_input(map: u64) -> Value {
        json!({"map": map, "text": "a node", "kind": "note"})
    }

    /// CON-114: at the limit a node is refused, naming the count and the limit; under it, it passes.
    #[test]
    fn a_node_past_the_limit_is_refused_naming_the_count() {
        let memory = memory_with_map();
        let limited = tool(memory.clone(), "mindmap_add", 2).unwrap();
        // The map holds its root: one node. Under the limit of 2, one more passes.
        assert!(now(Box::pin(async { limited.check_node_limit(&add_input(1)).await })).is_ok());
        now(memory.map_add(1, NewMapNode { text: "first".into(), kind: "note".into(), ..Default::default() })).unwrap();
        // Two nodes now: the limit is reached, so the next is refused.
        let err = now(Box::pin(async { limited.check_node_limit(&add_input(1)).await })).unwrap_err().to_string();
        assert!(err.contains("holds 2 nodes"), "{err}");
        assert!(err.contains("limit is 2"), "{err}");
        assert!(err.contains("nothing was added"), "{err}");
        assert!(err.contains("the user's to change"), "{err}");
        // Nothing was added by the refusal: the map still holds two.
        assert_eq!(now(memory.map_read(1)).unwrap().unwrap().nodes.len(), 2);
    }

    /// CON-114: 0 never refuses, and the check applies to the two adds alone.
    #[test]
    fn zero_never_refuses_and_only_the_adds_are_checked() {
        let memory = memory_with_map();
        for _ in 0..5 {
            now(memory.map_add(1, NewMapNode { text: "n".into(), kind: "note".into(), ..Default::default() })).unwrap();
        }
        let none = tool(memory.clone(), "mindmap_add", 0).unwrap();
        assert!(now(Box::pin(async { none.check_node_limit(&add_input(1)).await })).is_ok(), "0 is no limit");
        let limited = tool(memory.clone(), "flowchart_add", 3).unwrap();
        assert!(now(Box::pin(async { limited.check_node_limit(&add_input(1)).await })).is_err(), "flowchart_add refused too, 6 held");
    }

    /// The limit is checked on the map the node is added to, and an unknown map is left to the add.
    #[test]
    fn an_unknown_map_is_left_to_the_add() {
        let memory = memory_with_map();
        let limited = tool(memory, "mindmap_add", 1).unwrap();
        assert!(now(Box::pin(async { limited.check_node_limit(&add_input(99)).await })).is_ok());
    }
}
