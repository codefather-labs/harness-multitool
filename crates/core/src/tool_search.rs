//! Deferred tools and tool_search (CON-201, journal#1663). A deferred tool, an MCP server's, is
//! not in a request until the conversation has found it or called it: rea alone lists ~208k
//! tokens of schemas (journal#1662). tool_search finds them, as Claude Code's ToolSearch does.
//!
//! What a conversation has found is worked out again from its history, every search being run
//! again over the same tools, so a resumed conversation keeps its tools without a store of its own.

use harness_api::{Content, Message, ToolSpec};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub const SEARCH_TOOL: &str = "tool_search";
/// How many tools a keyword search gives when it is not told.
const DEFAULT_RESULTS: usize = 5;

/// tool_search's spec: what it does, and the name of every deferred tool, by server.
pub fn spec(deferred: &[ToolSpec]) -> ToolSpec {
    let mut by_server: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for t in deferred {
        let (server, tool) = match t.name.strip_prefix("mcp__").and_then(|rest| rest.split_once("__")) {
            Some((server, tool)) => (format!("mcp__{server}__"), tool.to_string()),
            None => (String::new(), t.name.clone()),
        };
        by_server.entry(server).or_default().push(tool);
    }
    let names: String = by_server.iter().map(|(server, tools)| format!("\n- {server}: {}", tools.join(", "))).collect();
    ToolSpec {
        name: SEARCH_TOOL.into(),
        description: format!(
            "Find tools that are not loaded yet and load them: they are usable from then on. Query \"select:<name>,<name>\" loads those tools by their full names; keywords find tools by name and description, best first. Answers with each tool's name, description and input schema; then call it by its name. Not loaded yet, each name being the prefix and the tool:{names}"
        ),
        schema: json!({"type": "object", "properties": {"query": {"type": "string", "description": "\"select:<name>,<name>\", or keywords."}, "max_results": {"type": "integer", "description": "How many tools keywords give, 5 if not said."}}, "required": ["query"]}),
    }
}

/// The tools a query finds among `deferred`.
pub fn search<'a>(deferred: &'a [ToolSpec], input: &Value) -> Vec<&'a ToolSpec> {
    let query = input["query"].as_str().unwrap_or_default().trim();
    if let Some(names) = query.strip_prefix("select:") {
        // A name as the description groups it, `mcp__rea__: tool`, is read as `mcp__rea__tool`: a
        // small model copies the grouping (gemma4:e2b did, journal#1670).
        let wanted: Vec<String> = names.split(',').map(|n| n.trim().replace("__:", "__").replace(' ', "")).filter(|n| !n.is_empty()).collect();
        return wanted.iter().filter_map(|w| deferred.iter().find(|t| t.name == *w).or_else(|| deferred.iter().find(|t| t.name.ends_with(&format!("__{w}"))))).collect();
    }
    let max = input["max_results"].as_u64().map_or(DEFAULT_RESULTS, |m| m.clamp(1, 50) as usize);
    let terms: Vec<String> = query.to_lowercase().split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()).map(String::from).collect();
    let mut scored: Vec<(usize, &ToolSpec)> = deferred
        .iter()
        .map(|t| {
            let (name, description) = (t.name.to_lowercase(), t.description.to_lowercase());
            let score = terms.iter().map(|term| if name.contains(term.as_str()) { 3 } else if description.contains(term.as_str()) { 1 } else { 0 }).sum();
            (score, t)
        })
        .filter(|(score, _)| *score > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
    scored.into_iter().take(max).map(|(_, t)| t).collect()
}

/// tool_search's answer.
pub fn answer(found: &[&ToolSpec]) -> String {
    if found.is_empty() {
        return "No tool matched. Try other keywords, or select:<name> with a name from tool_search's description.".into();
    }
    let tools: Vec<Value> = found.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    format!("Loaded; call them by name.\n{}", json!(tools))
}

/// The deferred tools a conversation has found or called.
pub fn found(messages: &[Message], deferred: &[ToolSpec]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for message in messages {
        for content in &message.content {
            if let Content::ToolUse { name, input, .. } = content {
                if name == SEARCH_TOOL {
                    names.extend(search(deferred, input).into_iter().map(|t| t.name.clone()));
                } else if deferred.iter().any(|t| &t.name == name) {
                    names.insert(name.clone());
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, description: &str) -> ToolSpec {
        ToolSpec { name: name.into(), description: description.into(), schema: json!({"type": "object"}) }
    }

    #[test]
    fn select_and_keywords_find_and_history_remembers() {
        let deferred = [tool("mcp__rea__list_architectures", "Lists the CPU architectures"), tool("mcp__rea__open_binary", "Opens a binary"), tool("mcp__web__open_page", "Opens a page in a browser")];
        let s = spec(&deferred);
        assert!(s.description.contains("- mcp__rea__: list_architectures, open_binary") && s.description.contains("- mcp__web__: open_page"), "{}", s.description);
        let names = |input: Value| search(&deferred, &input).iter().map(|t| t.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(json!({"query": "select:mcp__web__open_page, list_architectures, nope"})), ["mcp__web__open_page", "mcp__rea__list_architectures"]);
        assert_eq!(names(json!({"query": "select:mcp__rea__:list_architectures, mcp__web__: open_page"})), ["mcp__rea__list_architectures", "mcp__web__open_page"], "the description's grouping, copied");
        assert_eq!(names(json!({"query": "open browser"})), ["mcp__web__open_page", "mcp__rea__open_binary"], "a name and a description beat a name");
        assert_eq!(names(json!({"query": "open", "max_results": 1})).len(), 1);
        assert!(names(json!({"query": "zzz"})).is_empty());
        assert!(answer(&[]).starts_with("No tool matched"));
        assert!(answer(&search(&deferred, &json!({"query": "select:open_binary"}))).contains("\"input_schema\""));
        let said = Message { role: harness_api::Role::Assistant, content: vec![Content::ToolUse { id: "1".into(), name: SEARCH_TOOL.into(), input: json!({"query": "select:open_binary"}) }, Content::ToolUse { id: "2".into(), name: "mcp__web__open_page".into(), input: json!({}) }, Content::ToolUse { id: "3".into(), name: "read".into(), input: json!({}) }] };
        assert_eq!(found(&[said], &deferred).into_iter().collect::<Vec<_>>(), ["mcp__rea__open_binary", "mcp__web__open_page"]);
    }
}
