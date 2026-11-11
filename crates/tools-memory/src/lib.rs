//! The agent's tools over `Memory` (IF-4, IF-7). A mind map and a flowchart are separate tools
//! (ADR 0011). Each result is JSON: the model reads it, and the frontend draws it as a card.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use harness_api::*;
use serde_json::{Value, json};

/// A list of strings. A model often gives one as a bare string: it counts as that one, not as nothing,
/// or a proof or a flag would be lost with no word said (journal#683).
fn strings(v: &Value) -> Vec<String> {
    if let Some(one) = v.as_str() {
        return if one.trim().is_empty() { Vec::new() } else { vec![one.to_string()] };
    }
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

/// Where a project's proofs live (journal#928): one folder per project, in its own `.harness`.
pub const PROOFS_DIR: &str = ".harness/proofs";
/// Where a proof from elsewhere is copied to, inside it.
const PROOFS_COPIED: &str = "journal";
/// The largest file copied in as a proof.
pub const PROOF_MAX_BYTES: u64 = 50 << 20;

fn resolve(file: &str, root: &str) -> PathBuf {
    let file = file.trim();
    match file.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME").map_or_else(|| PathBuf::from(file), |home| PathBuf::from(home).join(rest)),
        None if Path::new(file).is_absolute() => PathBuf::from(file),
        None => Path::new(root).join(file),
    }
}

/// Every proof file goes in the project's `.harness/proofs/` (journal#928): one there already is
/// named by its path in the project; one from elsewhere is copied in, to `.harness/proofs/journal/`,
/// and named by its copy, so what the journal links to lasts with the project. A folder, or a file
/// over 50 MB, is refused, saying why. Answers the input with `proof_file` so named.
fn place_proofs(input: &Value, root: &str) -> PluginResult<Value> {
    check_proofs(input, root)?;
    let files = strings(&input["proof_file"]);
    if files.is_empty() {
        return Ok(input.clone());
    }
    let proofs = Path::new(root).join(PROOFS_DIR);
    let inside = proofs.canonicalize().ok();
    let mut placed = Vec::new();
    for file in files {
        let path = resolve(&file, root);
        let real = path.canonicalize().map_err(|e| PluginError::new(format!("proof_file `{file}`: {e}")))?;
        if inside.as_ref().is_some_and(|inside| real.starts_with(inside)) {
            let within = real.strip_prefix(inside.as_ref().expect("checked")).expect("checked");
            placed.push(Path::new(PROOFS_DIR).join(within).to_string_lossy().into_owned());
            continue;
        }
        let meta = std::fs::metadata(&real).map_err(|e| PluginError::new(format!("proof_file `{file}`: {e}")))?;
        if meta.is_dir() {
            return Err(PluginError::new(format!("proof_file `{file}` is a folder: name the files in it, and keep them in {PROOFS_DIR}/ of the project")));
        }
        if meta.len() > PROOF_MAX_BYTES {
            return Err(PluginError::new(format!("proof_file `{file}` is {} MB, over the {} MB a proof is copied at: keep a smaller proof in {PROOFS_DIR}/ of the project", meta.len() >> 20, PROOF_MAX_BYTES >> 20)));
        }
        let into = proofs.join(PROOFS_COPIED);
        std::fs::create_dir_all(&into).map_err(|e| PluginError::new(format!("making {}: {e}", into.display())))?;
        let name = real.file_name().map_or_else(|| "proof".to_string(), |n| n.to_string_lossy().into_owned());
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
            _ => (name.clone(), String::new()),
        };
        // The same file again is the same copy; another of the same name gets a number.
        let content = std::fs::read(&real).map_err(|e| PluginError::new(format!("reading proof_file `{file}`: {e}")))?;
        let mut n = 0;
        let target = loop {
            let candidate = into.join(if n == 0 { name.clone() } else { format!("{stem}-{n}{extension}") });
            match std::fs::read(&candidate) {
                Ok(there) if there == content => break candidate,
                Ok(_) => n += 1,
                Err(_) => {
                    std::fs::write(&candidate, &content).map_err(|e| PluginError::new(format!("copying proof_file `{file}` to {}: {e}", candidate.display())))?;
                    break candidate;
                }
            }
        };
        placed.push(Path::new(PROOFS_DIR).join(PROOFS_COPIED).join(target.file_name().expect("a file")).to_string_lossy().into_owned());
    }
    let mut input = input.clone();
    input["proof_file"] = json!(placed);
    Ok(input)
}

/// The proof files of a todo's whole branch, as the project has them (journal#928).
async fn branch_proof_files(memory: &dyn Memory, id: u64, root: &str) -> Vec<Value> {
    let Ok(branch) = memory.more(MemoryOp::JournalBranch { id }).await else { return Vec::new() };
    let mut out = Vec::new();
    for entry in branch["entries"].as_array().into_iter().flatten() {
        let Some(id) = entry["entry"]["id"].as_u64() else { continue };
        let Ok(Some(record)) = memory.journal_get(id).await else { continue };
        for proof in &record.proofs {
            if resolve(proof, root).is_file() {
                out.push(json!({"entry": id, "file": proof}));
            }
        }
    }
    out
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

/// A number that may be left out; anything else there is refused.
fn optional_number(input: &Value, name: &str, default: u64) -> PluginResult<u64> {
    if input[name].is_null() { Ok(default) } else { number(input, name) }
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
    /// The most linked entries an answer holds: the `search_max_linked` setting, 0 for no paging (CON-118).
    search_max_linked: u64,
}

/// How far a graph is followed when nobody says (CON-118).
const GRAPH_DEPTH: u64 = 2;

impl MemoryTool {
    /// Adds to a found entry's answer what is linked to it (CON-118). The entries stand whatever this says: a
    /// memory that keeps no links adds nothing.
    async fn add_graph(&self, answer: &mut Value, input: &Value) -> PluginResult<()> {
        let ids: Vec<u64> = match self.name {
            "journal_search" => answer["entries"].as_array().map_or(Vec::new(), |entries| entries.iter().filter_map(|e| e["id"].as_u64()).collect()),
            _ => answer["id"].as_u64().into_iter().collect(),
        };
        let depth = optional_number(input, "depth", GRAPH_DEPTH)?;
        let offset = optional_number(input, "offset", 0)? as usize;
        let op = MemoryOp::JournalGraph { ids, depth, offset, limit: self.search_max_linked as usize };
        if let (Ok(Value::Object(page)), Some(whole)) = (self.memory.more(op).await, answer.as_object_mut()) {
            whole.extend(page);
        }
        Ok(())
    }

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
            // A proof named as a file must be there before the entry is stored (journal#607), and
            // in the project's proofs folder, copied in if it is elsewhere (journal#928).
            let placed;
            let input = if self.name == "journal_add" {
                match place_proofs(input, &cx.root) {
                    Ok(v) => {
                        placed = v;
                        &placed
                    }
                    Err(e) => return ToolOutput::error(e.to_string()),
                }
            } else {
                input
            };
            if matches!(self.name, "mindmap_add" | "flowchart_add") && let Err(e) = self.check_node_limit(input).await {
                return ToolOutput::error(e.to_string());
            }
            let mut answer = (self.action)(self.memory.as_ref(), input).await;
            if matches!(self.name, "journal_search" | "journal_get") && let Ok(found) = &mut answer && let Err(e) = self.add_graph(found, input).await {
                return ToolOutput::error(e.to_string());
            }
            // A todo's answer names every proof file of its branch (journal#928).
            if matches!(self.name, "journal_get" | "journal_branch") && let Ok(found) = &mut answer && let Some(id) = input["id"].as_u64() {
                let files = branch_proof_files(self.memory.as_ref(), id, &cx.root).await;
                if !files.is_empty() {
                    found["proof_files"] = json!(files);
                }
            }
            output(answer)
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

/// A category that is itself a flag sets it: an entry of these kinds is open work (todo, question,
/// problem) or binds the project (invariant, principle), and without its flag nothing would show it.
fn flags_with_category(category: &str, mut flags: Vec<String>) -> Vec<String> {
    if ["todo", "question", "problem", "invariant", "principle"].contains(&category) && !flags.iter().any(|f| f == category) {
        flags.push(category.to_string());
    }
    flags
}

/// Every memory tool, by name. `map_max_nodes` is the most nodes a map may hold, 0 for none (CON-114);
/// `search_max_linked` the most linked entries an answer holds, 0 for no paging (CON-118).
pub fn tool(memory: Rc<dyn Memory>, name: &str, map_max_nodes: u64, search_max_linked: u64) -> Option<MemoryTool> {
    let tool = |name, description, schema: fn() -> Value, action: Action| Some(MemoryTool { memory: memory.clone(), name, description, schema, action, map_max_nodes, search_max_linked });
    match name {
        "journal_add" => tool(
            "journal_add",
            "Record a decision, finding or update in the project's journal, one per entry, with the why. The category says what kind of entry it is. Flags are optional, any of: todo, question, problem, unverified, verified, specification, constraint, invariant, principle, important, idea, concept, comment, deprecated. An invariant or principle entry is mirrored to docs/spec/invariants.md or docs/spec/principles.md and needs its doc id (INV- or P-).",
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
                    let flags = flags_with_category(&category, strings(&input["flags"]));
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
            "Search the project's journal by words for earlier decisions and findings, before deciding something or redoing work. Answers with the closest entries whole, and `linked`: what links reach from them, each only as id, title and link, nearest first. `depth` says how many hops, 2 by default; when `next_offset` is there, the same call with `offset` gives the next page. Trust a replaced or deprecated entry only as history.",
            || object(json!({"query": {"type": "string"}, "depth": {"type": "integer"}, "offset": {"type": "integer"}}), &["query"]),
            |memory, input| {
                Box::pin(async move {
                    let found = memory.journal_search(require(input, "query")?).await?;
                    Ok(json!({"entries": found}))
                })
            },
        ),
        "journal_get" => tool(
            "journal_get",
            "Read one journal entry in full, with its flags, tags and what blocks it, by id, and `linked`: what links reach from it, each only as id, title and link. `depth` and `offset` as in journal_search. Use it on what journal_agenda or journal_list show only the start of, and before relying on an entry.",
            || object(json!({"id": {"type": "integer"}, "depth": {"type": "integer"}, "offset": {"type": "integer"}}), &["id"]),
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
            "What is open in the journal, flag by flag: todos, questions, problems, and the specifications, constraints, invariants and principles that bind the work, each entry as its start: journal_get reads one whole. Read it on a fresh context. Those groups hold what can be worked on now; `blocked` is what waits for another entry, and `blocker_dropped` is what waited for something dropped instead of solved: re-read those, then lift the link with journal_mark.",
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
    fn a_category_that_is_a_flag_sets_it() {
        let s = |v: &[&str]| v.iter().map(|f| f.to_string()).collect::<Vec<_>>();
        assert_eq!(flags_with_category("invariant", vec![]), s(&["invariant"]));
        assert_eq!(flags_with_category("principle", s(&["important"])), s(&["important", "principle"]));
        assert_eq!(flags_with_category("invariant", s(&["invariant"])), s(&["invariant"]), "already set, not twice");
        assert_eq!(flags_with_category("decision", vec![]), Vec::<String>::new());
    }

    /// journal#928: proofs live in the project's .harness/proofs/; one from elsewhere is copied in
    /// and named by its copy; the same file is one copy, another of its name gets a number; a
    /// folder or a file too big is refused, saying why.
    #[test]
    fn proofs_go_in_the_projects_proofs_folder() {
        let root = scratch("placed");
        let outside = scratch("placed-outside");
        std::fs::create_dir_all(root.join(".harness/proofs/916")).unwrap();
        std::fs::write(root.join(".harness/proofs/916/run.txt"), "in").unwrap();
        std::fs::write(outside.join("shot.png"), "png one").unwrap();
        let r = root.to_str().unwrap();
        let placed = place_proofs(&with_files(&[".harness/proofs/916/run.txt", outside.join("shot.png").to_str().unwrap()]), r).unwrap();
        assert_eq!(placed["proof_file"], json!([".harness/proofs/916/run.txt", ".harness/proofs/journal/shot.png"]));
        assert_eq!(std::fs::read_to_string(root.join(".harness/proofs/journal/shot.png")).unwrap(), "png one");
        assert_eq!(placed["proof"], json!(["any words"]), "the words stay as they were");
        let again = place_proofs(&with_files(&[outside.join("shot.png").to_str().unwrap()]), r).unwrap();
        assert_eq!(again["proof_file"], json!([".harness/proofs/journal/shot.png"]), "the same file is the same copy");
        std::fs::write(outside.join("shot.png"), "png two").unwrap();
        let other = place_proofs(&with_files(&[outside.join("shot.png").to_str().unwrap()]), r).unwrap();
        assert_eq!(other["proof_file"], json!([".harness/proofs/journal/shot-1.png"]), "another of its name gets a number");
        let folder = place_proofs(&with_files(&[outside.to_str().unwrap()]), r).unwrap_err();
        assert!(folder.to_string().contains("is a folder"), "{folder}");
        let big = outside.join("big.bin");
        std::fs::File::create(&big).unwrap().set_len(PROOF_MAX_BYTES + 1).unwrap();
        let refused = place_proofs(&with_files(&[big.to_str().unwrap()]), r).unwrap_err();
        assert!(refused.to_string().contains("over the 50 MB"), "{refused}");
        assert!(place_proofs(&with_files(&["missing.txt"]), r).is_err(), "a missing file is still refused");
        assert_eq!(place_proofs(&json!({"proof": ["words only"]}), r).unwrap(), json!({"proof": ["words only"]}));
    }

    /// A model gave `proof_file` as one string: the check held for nothing, and the proof was lost.
    #[test]
    fn a_bare_string_counts_as_one_item() {
        assert_eq!(strings(&json!("one")), ["one"]);
        assert_eq!(strings(&json!(["a", "b"])), ["a", "b"]);
        assert!(strings(&json!("")).is_empty() && strings(&json!(null)).is_empty());
        assert!(check_proofs(&json!({"proof_file": "missing/out.txt"}), "/nonexistent-root").is_err());
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

    /// CON-118: what the tool asks of the store, and what it answers, without one.
    #[derive(Default)]
    struct Graphs {
        asked: std::cell::RefCell<Vec<MemoryOp>>,
    }
    impl Memory for Graphs {
        fn journal_add(&self, _: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
            unused()
        }
        fn journal_search(&self, _: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
            Box::pin(std::future::ready(Ok(vec![JournalRecord { id: 7, body: "whole".into(), ..Default::default() }])))
        }
        fn journal_get(&self, id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
            Box::pin(std::future::ready(Ok(Some(JournalRecord { id, body: "whole".into(), ..Default::default() }))))
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
        fn map_new(&self, _: MapKind, _: &str, _: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
            unused()
        }
        fn map_add(&self, _: u64, _: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
            unused()
        }
        fn map_read(&self, _: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
            unused()
        }
        fn more(&self, op: MemoryOp) -> BoxFuture<'_, PluginResult<Value>> {
            self.asked.borrow_mut().push(op);
            Box::pin(std::future::ready(Ok(json!({"linked": [{"id": 8, "title": "t", "via": "#8 cites #7"}], "linked_total": 1, "linked_left": 0}))))
        }
    }

    fn answer_of(found: &MemoryTool, input: &Value) -> PluginResult<Value> {
        now(Box::pin(async {
            let mut answer = (found.action)(found.memory.as_ref(), input).await?;
            found.add_graph(&mut answer, input).await?;
            Ok(answer)
        }))
    }

    #[test]
    fn search_and_get_answer_their_graph_and_pass_depth_offset_and_page() {
        let memory = Rc::new(Graphs::default());
        for (name, input) in [("journal_search", json!({"query": "q"})), ("journal_get", json!({"id": 7}))] {
            let text = answer_of(&tool(memory.clone(), name, 200, 100).unwrap(), &input).unwrap().to_string();
            assert!(text.contains("#8 cites #7") && text.contains("whole"), "{text}");
        }
        answer_of(&tool(memory.clone(), "journal_get", 200, 5).unwrap(), &json!({"id": 7, "depth": 3, "offset": 10})).unwrap();
        let asked = memory.asked.borrow();
        assert!(matches!(&asked[0], MemoryOp::JournalGraph { ids, depth: 2, offset: 0, limit: 100 } if ids == &vec![7]), "{:?}", asked[0]);
        assert!(matches!(&asked[2], MemoryOp::JournalGraph { depth: 3, offset: 10, limit: 5, .. }), "{:?}", asked[2]);
    }

    #[test]
    fn a_depth_that_is_no_number_is_refused() {
        let found = tool(Rc::new(Graphs::default()), "journal_search", 200, 100).unwrap();
        let error = answer_of(&found, &json!({"query": "q", "depth": "all"})).unwrap_err().to_string();
        assert!(error.contains("depth must be a number"), "{error}");
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
        let limited = tool(memory.clone(), "mindmap_add", 2, 100).unwrap();
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
        let none = tool(memory.clone(), "mindmap_add", 0, 100).unwrap();
        assert!(now(Box::pin(async { none.check_node_limit(&add_input(1)).await })).is_ok(), "0 is no limit");
        let limited = tool(memory.clone(), "flowchart_add", 3, 100).unwrap();
        assert!(now(Box::pin(async { limited.check_node_limit(&add_input(1)).await })).is_err(), "flowchart_add refused too, 6 held");
    }

    /// The limit is checked on the map the node is added to, and an unknown map is left to the add.
    #[test]
    fn an_unknown_map_is_left_to_the_add() {
        let memory = memory_with_map();
        let limited = tool(memory, "mindmap_add", 1, 100).unwrap();
        assert!(now(Box::pin(async { limited.check_node_limit(&add_input(99)).await })).is_ok());
    }
}
