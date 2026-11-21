//! The names of tools across adapters (CON-191). Each program names its tools its own way, and a
//! model imitates the history it is given: a conversation moved from Claude Code to the own loop
//! holds `Read {file_path}` where the own loop has `read {path}`. One table maps a past call and its
//! arguments, so a request or a form speaks the names of the program it goes to. The log keeps
//! what was said (INV-5).

use serde_json::{Map, Value, json};

/// Claude Code's built-in tools and the own loop's, with their argument names: (Claude Code, own
/// loop, [(Claude Code's argument, the own loop's)]).
/// A pair of tool names with its argument names: (Claude Code, own loop, [(Claude Code's, own)]).
type Pair = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

const PAIRS: &[Pair] = &[
    ("Read", "read", &[("file_path", "path")]),
    ("Edit", "edit", &[("file_path", "path"), ("old_string", "old_text"), ("new_string", "new_text")]),
    ("Bash", "shell", &[("command", "command"), ("run_in_background", "run_in_background")]),
    ("Grep", "search", &[("pattern", "text"), ("path", "path")]),
    ("WebFetch", "fetch", &[("url", "url")]),
];

/// The prefixes the harness's MCP servers give its tools in Claude Code.
const HARNESS_PREFIX: &str = "mcp__harness__";
const MODE_PREFIX: &str = "mcp__mode__";

fn renamed(input: &Value, args: &[(&str, &str)], forward: bool) -> Value {
    let Some(object) = input.as_object() else { return input.clone() };
    let mut out = Map::new();
    for (key, value) in object {
        let to = args.iter().find(|(a, b)| if forward { a == key } else { b == key }).map(|(a, b)| if forward { *b } else { *a });
        // An argument the other side has no word for is left out: it would only mislead.
        if let Some(to) = to {
            out.insert(to.to_string(), value.clone());
        }
    }
    Value::Object(out)
}

/// A Claude Code call as the own loop names it, when there is an equivalent.
pub fn to_own_loop(name: &str, input: &Value) -> Option<(String, Value)> {
    if let Some((_, own, args)) = PAIRS.iter().find(|(cc, _, _)| *cc == name) {
        return Some((own.to_string(), renamed(input, args, true)));
    }
    if name == "Write" {
        // The own loop's edit with old_text empty writes the whole file.
        return Some(("edit".to_string(), json!({"path": input["file_path"], "old_text": "", "new_text": input["content"]})));
    }
    if let Some(rest) = name.strip_prefix(MODE_PREFIX) {
        return Some((format!("mode__{rest}"), input.clone()));
    }
    if let Some(rest) = name.strip_prefix(HARNESS_PREFIX) {
        return Some((rest.to_string(), input.clone()));
    }
    None
}

/// An own-loop call as Claude Code names it. A name already Claude Code's stays.
pub fn to_claude_code(name: &str, input: &Value) -> (String, Value) {
    if let Some((cc, _, args)) = PAIRS.iter().find(|(_, own, _)| *own == name) {
        return (cc.to_string(), renamed(input, args, false));
    }
    let claude_codes = name.starts_with("mcp__") || name.chars().next().is_some_and(char::is_uppercase);
    if claude_codes {
        return (name.to_string(), input.clone());
    }
    if let Some(rest) = name.strip_prefix("mode__") {
        return (format!("{MODE_PREFIX}{rest}"), input.clone());
    }
    (format!("{HARNESS_PREFIX}{name}"), input.clone())
}

/// For an unknown tool's answer: the tool this session has for it, with its arguments.
pub fn unknown_hint(name: &str, offered: &[&str]) -> Option<String> {
    let meant = to_own_loop(name, &Value::Null).map(|(n, _)| n).filter(|n| offered.contains(&n.as_str())).or_else(|| offered.iter().find(|o| o.eq_ignore_ascii_case(name)).map(|o| o.to_string()))?;
    let args = PAIRS.iter().find(|(_, own, _)| *own == meant).map(|(_, _, args)| args.iter().map(|(_, b)| *b).collect::<Vec<_>>().join(", "));
    Some(match args {
        Some(args) => format!("this session's tool for it is {meant} {{{args}}}"),
        None => format!("this session's tool for it is {meant}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_pair_maps_both_ways_with_its_arguments() {
        let cases = [
            ("Read", json!({"file_path": "/p/a.txt"}), "read", json!({"path": "/p/a.txt"})),
            ("Edit", json!({"file_path": "a", "old_string": "x", "new_string": "y"}), "edit", json!({"path": "a", "old_text": "x", "new_text": "y"})),
            ("Bash", json!({"command": "ls", "run_in_background": true}), "shell", json!({"command": "ls", "run_in_background": true})),
            ("Grep", json!({"pattern": "alpha", "path": "src"}), "search", json!({"text": "alpha", "path": "src"})),
            ("WebFetch", json!({"url": "http://x"}), "fetch", json!({"url": "http://x"})),
            ("mcp__harness__journal_add", json!({"body": "b"}), "journal_add", json!({"body": "b"})),
            ("mcp__mode__plan_enter", json!({}), "mode__plan_enter", json!({})),
        ];
        for (cc, cc_in, own, own_in) in cases {
            assert_eq!(to_own_loop(cc, &cc_in), Some((own.to_string(), own_in.clone())), "{cc}");
            assert_eq!(to_claude_code(own, &own_in), (cc.to_string(), cc_in.clone()), "{own}");
        }
    }

    #[test]
    fn what_has_no_word_on_the_other_side() {
        // Write becomes an edit with old_text empty, one way.
        assert_eq!(to_own_loop("Write", &json!({"file_path": "n.txt", "content": "hi"})), Some(("edit".to_string(), json!({"path": "n.txt", "old_text": "", "new_text": "hi"}))));
        // An argument with no equivalent is left out; a name with no row stays.
        assert_eq!(to_own_loop("WebFetch", &json!({"url": "u", "prompt": "summarise"})).unwrap().1, json!({"url": "u"}));
        assert_eq!(to_own_loop("Glob", &json!({"pattern": "*"})), None);
        assert_eq!(to_own_loop("read", &json!({"path": "a"})), None, "already the own loop's");
        // Claude Code's own names stay as they are going to Claude Code.
        assert_eq!(to_claude_code("Read", &json!({"file_path": "a"})).0, "Read");
        assert_eq!(to_claude_code("mcp__harness__journal_get", &json!({})).0, "mcp__harness__journal_get");
        assert_eq!(to_claude_code("set_nick", &json!({"nick": "x"})).0, "mcp__harness__set_nick");
    }

    #[test]
    fn an_unknown_tool_is_answered_with_the_one_meant() {
        let offered = ["read", "edit", "shell", "journal_add"];
        assert_eq!(unknown_hint("Read", &offered).as_deref(), Some("this session's tool for it is read {path}"));
        assert_eq!(unknown_hint("mcp__harness__journal_add", &offered).as_deref(), Some("this session's tool for it is journal_add"));
        assert_eq!(unknown_hint("SHELL", &offered).as_deref(), Some("this session's tool for it is shell {command, run_in_background}"));
        assert_eq!(unknown_hint("Glob", &offered), None);
    }
}
