//! Which MCP servers Claude Code and Codex would run in a project (CON-201), read from their own
//! files: nothing is started here.
//!
//! The order is the order of precedence, and the first server of a name wins:
//! 1. the harness's own (`[session.mcp_servers]`);
//! 2. Claude Code's local scope, `projects[<root>].mcpServers` of `.claude.json`;
//! 3. the project's `.mcp.json`, only the servers approved for it, as Claude Code asks for them;
//! 4. Claude Code's user scope, `mcpServers` of `.claude.json`;
//! 5. Claude Code's enabled plugins, as `plugin_<plugin>_<server>`, Claude Code's own name for them;
//! 6. Codex's `config.toml`, `[mcp_servers]`.
//!
//! Only stdio servers (a `command`) are taken; one over HTTP or SSE is not (journal#1660).

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One MCP server, as it would be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// Its name in tool names, `mcp__<name>__<tool>`: only `[A-Za-z0-9_-]`.
    pub name: String,
    /// Where it was found, for the user to see: `claude: user`, `plugin chrome-devtools-mcp`…
    pub source: String,
    pub command: String,
    pub args: Vec<String>,
    /// What it gets over the environment it starts in.
    pub env: BTreeMap<String, String>,
}

/// The files the servers are read from.
#[derive(Debug, Clone, Default)]
pub struct Places {
    /// The project's root, as Claude Code keys it in `.claude.json`.
    pub root: String,
    /// Claude Code's folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
    pub claude_dir: Option<PathBuf>,
    /// The `.claude.json` files, in order: the user's own, then the profile's over it.
    pub claude_jsons: Vec<PathBuf>,
    /// Codex's folder: `CODEX_HOME`, else `~/.codex`.
    pub codex_home: Option<PathBuf>,
    /// The harness's own servers, `{name: {command, args, env}}`.
    pub own: BTreeMap<String, Value>,
}

impl Places {
    /// Where they are for a session whose environment `read` gives (the window's), in `root`.
    /// `users_claude_json` names the user's own `.claude.json` when the window says it
    /// (`MULTITOOL_USERS_CLAUDE_JSON`, CON-145).
    pub fn from_env(read: &dyn Fn(&str) -> Option<String>, root: &str, own: BTreeMap<String, Value>) -> Places {
        let set = |name: &str| read(name).filter(|v| !v.is_empty());
        let home = set("HOME").or_else(|| set("USERPROFILE")).map(PathBuf::from);
        let claude_dir = set("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".claude")));
        let mut claude_jsons = Vec::new();
        let users = set("MULTITOOL_USERS_CLAUDE_JSON").map(PathBuf::from).or_else(|| match set("CLAUDE_CONFIG_DIR") {
            Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
            None => home.as_ref().map(|h| h.join(".claude.json")),
        });
        claude_jsons.extend(users);
        if let Some(profile) = set("CLAUDE_CONFIG_DIR").map(|d| PathBuf::from(d).join(".claude.json"))
            && !claude_jsons.contains(&profile)
        {
            claude_jsons.push(profile);
        }
        let codex_home = set("CODEX_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".codex")));
        Places { root: root.to_string(), claude_dir, claude_jsons, codex_home, own }
    }
}

/// A name as Claude Code puts it in a tool's name: what is not a letter, a digit, `_` or `-`
/// becomes `_`.
pub fn tool_safe(name: &str) -> String {
    name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

fn json_file(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `${NAME}` and `${NAME:-default}` in `text`, from `vars`; a name it does not know is left.
pub fn expand(text: &str, vars: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 2..];
        let Some(end) = after.find('}') else {
            out.push_str(&rest[at..]);
            return out;
        };
        let inner = &after[..end];
        let (name, default) = match inner.split_once(":-") {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        match vars(name).or_else(|| default.map(String::from)) {
            Some(value) => out.push_str(&value),
            None => out.push_str(&rest[at..at + 3 + end]),
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A server from its configuration, `{command, args, env}`, or None when it has no command (HTTP).
fn stdio(name: &str, source: &str, config: &Value, vars: &dyn Fn(&str) -> Option<String>) -> Option<Server> {
    if config.get("type").and_then(Value::as_str).is_some_and(|t| t != "stdio") {
        return None;
    }
    let command = config.get("command")?.as_str()?;
    let args = config.get("args").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(|s| expand(s, vars)).collect()).unwrap_or_default();
    let env = config.get("env").and_then(Value::as_object).map(|e| e.iter().filter_map(|(k, v)| Some((k.clone(), expand(v.as_str()?, vars)))).collect()).unwrap_or_default();
    Some(Server { name: tool_safe(name), source: source.to_string(), command: expand(command, vars), args, env })
}

/// The servers of a `.mcp.json`-shaped file: `{mcpServers: {…}}`, or the map itself.
fn server_map(v: &Value) -> Map<String, Value> {
    match v.get("mcpServers").and_then(Value::as_object) {
        Some(map) => map.clone(),
        None => v.as_object().cloned().unwrap_or_default(),
    }
}

/// Claude Code's settings files for `root`, in order: the user's, the project's, its local one.
fn settings(places: &Places) -> Vec<Value> {
    let root = Path::new(&places.root);
    places.claude_dir.iter().map(|d| d.join("settings.json")).chain([root.join(".claude").join("settings.json"), root.join(".claude").join("settings.local.json")]).filter_map(|p| json_file(&p)).collect()
}

/// Whether a server of the project's `.mcp.json` was approved, as Claude Code keeps it: every one
/// (`enableAllProjectMcpServers`), or by name (`enabledMcpjsonServers`), and not refused
/// (`disabledMcpjsonServers`); in its settings or in the project's entry of `.claude.json`.
fn approved(name: &str, sources: &[Value]) -> bool {
    let names = |key: &str| sources.iter().any(|s| s[key].as_array().is_some_and(|a| a.iter().any(|n| n.as_str() == Some(name))));
    if names("disabledMcpjsonServers") {
        return false;
    }
    sources.iter().any(|s| s["enableAllProjectMcpServers"] == true) || names("enabledMcpjsonServers")
}

/// Every server found, by precedence; the first of a name wins.
pub fn servers(places: &Places, env: &dyn Fn(&str) -> Option<String>) -> Vec<Server> {
    let root = places.root.clone();
    let vars = |name: &str| if name == "CLAUDE_PROJECT_DIR" { Some(root.clone()) } else { env(name) };
    let mut found: Vec<Server> = Vec::new();
    let mut add = |server: Option<Server>| {
        if let Some(s) = server
            && !found.iter().any(|f| f.name == s.name)
            && s.name != "harness"
            && s.name != "mode"
        {
            found.push(s);
        }
    };
    for (name, config) in &places.own {
        add(stdio(name, "harness", config, &vars));
    }
    let jsons: Vec<Value> = places.claude_jsons.iter().filter_map(|p| json_file(p)).collect();
    // A profile's .claude.json is over the user's: its servers come first.
    for json in jsons.iter().rev() {
        for (name, config) in server_map(&json["projects"][&places.root]) {
            add(stdio(&name, "claude: local", &config, &vars));
        }
    }
    let settings = settings(places);
    if let Some(project) = json_file(&Path::new(&places.root).join(".mcp.json")) {
        let mut sources = settings.clone();
        sources.extend(jsons.iter().map(|j| j["projects"][&places.root].clone()));
        for (name, config) in server_map(&project) {
            if approved(&name, &sources) {
                add(stdio(&name, "claude: project .mcp.json", &config, &vars));
            }
        }
    }
    for json in jsons.iter().rev() {
        for (name, config) in json["mcpServers"].as_object().cloned().unwrap_or_default() {
            add(stdio(&name, "claude: user", &config, &vars));
        }
    }
    for plugin in plugins(places, &settings) {
        for server in plugin_servers(&plugin, &vars) {
            add(Some(server));
        }
    }
    if let Some(codex) = &places.codex_home {
        for server in codex_servers(&codex.join("config.toml"), &vars) {
            add(Some(server));
        }
    }
    found
}

/// A plugin of Claude Code that is on for the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    pub marketplace: String,
    /// Where it is installed: `CLAUDE_PLUGIN_ROOT`.
    pub root: PathBuf,
    /// Its data folder: `CLAUDE_PLUGIN_DATA`.
    pub data: PathBuf,
}

/// The plugins on for the project: `enabledPlugins` of the settings (a later file over an earlier
/// one), each where `installed_plugins.json` says, for the user or for this project.
pub fn enabled_plugins(places: &Places) -> Vec<Plugin> {
    plugins(places, &settings(places))
}

fn plugins(places: &Places, settings: &[Value]) -> Vec<Plugin> {
    let Some(dir) = &places.claude_dir else { return Vec::new() };
    let mut on: BTreeMap<String, bool> = BTreeMap::new();
    for s in settings {
        for (id, value) in s["enabledPlugins"].as_object().cloned().unwrap_or_default() {
            on.insert(id, value == true);
        }
    }
    let installed = json_file(&dir.join("plugins").join("installed_plugins.json")).unwrap_or_default();
    let root = Path::new(&places.root);
    on.into_iter()
        .filter(|(_, on)| *on)
        .filter_map(|(id, _)| {
            let (name, marketplace) = id.split_once('@')?;
            let entries = installed["plugins"][&id].as_array()?;
            let entry = entries.iter().find(|e| e["scope"] == "user").or_else(|| entries.iter().find(|e| e["projectPath"].as_str().is_some_and(|p| root.starts_with(p))))?;
            let at = PathBuf::from(entry["installPath"].as_str()?);
            Some(Plugin { name: name.to_string(), marketplace: marketplace.to_string(), data: dir.join("plugins").join("data").join(format!("{name}-{marketplace}")), root: at })
        })
        .collect()
}

/// A plugin's servers: its `plugin.json` `mcpServers` (a map, or a file by its path), else its
/// `.mcp.json`, else its `mcp.json`.
fn plugin_servers(plugin: &Plugin, vars: &dyn Fn(&str) -> Option<String>) -> Vec<Server> {
    let root = plugin.root.to_string_lossy().into_owned();
    let data = plugin.data.to_string_lossy().into_owned();
    let vars = |name: &str| match name {
        "CLAUDE_PLUGIN_ROOT" => Some(root.clone()),
        "CLAUDE_PLUGIN_DATA" => Some(data.clone()),
        _ => vars(name),
    };
    let declared = json_file(&plugin.root.join(".claude-plugin").join("plugin.json")).and_then(|p| p.get("mcpServers").cloned());
    let map = match declared {
        Some(Value::Object(map)) => map,
        Some(Value::String(file)) => json_file(&plugin.root.join(expand(&file, &vars))).map(|v| server_map(&v)).unwrap_or_default(),
        _ => [".mcp.json", "mcp.json"].iter().find_map(|f| json_file(&plugin.root.join(f))).map(|v| server_map(&v)).unwrap_or_default(),
    };
    map.iter()
        .filter_map(|(name, config)| {
            let mut server = stdio(&format!("plugin_{}_{}", plugin.name, name), &format!("plugin {}", plugin.name), config, &vars)?;
            server.env.entry("CLAUDE_PLUGIN_ROOT".into()).or_insert_with(|| root.clone());
            server.env.entry("CLAUDE_PLUGIN_DATA".into()).or_insert_with(|| data.clone());
            Some(server)
        })
        .collect()
}

/// Codex's servers, `[mcp_servers.<name>]` with `command`, `args` and `env`; one with
/// `enabled = false` is left out.
fn codex_servers(file: &Path, vars: &dyn Fn(&str) -> Option<String>) -> Vec<Server> {
    let Some(text) = std::fs::read_to_string(file).ok() else { return Vec::new() };
    let Ok(config) = text.parse::<toml::Table>() else { return Vec::new() };
    let Some(servers) = config.get("mcp_servers").and_then(toml::Value::as_table) else { return Vec::new() };
    servers
        .iter()
        .filter_map(|(name, server)| {
            let json = serde_json::to_value(server).ok()?;
            if json["enabled"] == false {
                return None;
            }
            stdio(name, "codex", &json, vars)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn every_source_is_found_by_precedence_and_disabled_ones_are_not() {
        let t = tempfile::tempdir().unwrap();
        let (claude, codex, root) = (t.path().join("claude"), t.path().join("codex"), t.path().join("app"));
        let root_s = root.to_string_lossy().into_owned();
        let users_json = t.path().join(".claude.json");
        write(
            &users_json,
            &json!({
                "mcpServers": {"rea": {"command": "npx", "args": ["-y", "rea"], "env": {"K": "${HOME_X:-dflt}"}}, "web": {"type": "http", "url": "https://x"}, "dup": {"command": "user-dup"}, "harness": {"command": "no"}},
                "projects": {root_s.clone(): {"mcpServers": {"dup": {"command": "local-dup"}}, "enabledMcpjsonServers": ["ok"]}}
            })
            .to_string(),
        );
        write(&root.join(".mcp.json"), &json!({"mcpServers": {"ok": {"command": "proj", "args": ["${CLAUDE_PROJECT_DIR}/x"]}, "unasked": {"command": "evil"}}}).to_string());
        write(&claude.join("settings.json"), &json!({"enabledPlugins": {"cd@m": true, "uv@m": true, "off@m": false, "gone@m": true}}).to_string());
        let installed = |name: &str| claude.join(format!("plugins/cache/m/{name}/1"));
        write(
            &claude.join("plugins/installed_plugins.json"),
            &json!({"version": 2, "plugins": {
                "cd@m": [{"scope": "user", "installPath": installed("cd")}],
                "uv@m": [{"scope": "project", "projectPath": t.path(), "installPath": installed("uv")}],
                "off@m": [{"scope": "user", "installPath": installed("off")}]
            }})
            .to_string(),
        );
        write(&installed("cd").join(".claude-plugin/plugin.json"), &json!({"name": "cd", "mcpServers": {"chrome-devtools": {"command": "npx", "args": ["${CLAUDE_PLUGIN_ROOT}/s.js"]}}}).to_string());
        write(&installed("cd").join(".mcp.json"), &json!({"ignored": {"command": "no"}}).to_string());
        write(&installed("uv").join(".mcp.json"), &json!({"browser-use": {"command": "uvx", "args": ["${CLAUDE_PLUGIN_DATA}"]}}).to_string());
        write(&installed("off").join(".mcp.json"), &json!({"x": {"command": "no"}}).to_string());
        write(&codex.join("config.toml"), "[mcp_servers.cx]\ncommand = \"codex-srv\"\nargs = [\"a\"]\n[mcp_servers.cx.env]\nT = \"1\"\n[mcp_servers.dead]\ncommand = \"d\"\nenabled = false\n");
        let places = Places { root: root_s.clone(), claude_dir: Some(claude.clone()), claude_jsons: vec![users_json], codex_home: Some(codex), own: [("mine".to_string(), json!({"command": "own"}))].into() };
        let found = servers(&places, &|_| None);
        let names: Vec<(&str, &str, &str)> = found.iter().map(|s| (s.name.as_str(), s.source.as_str(), s.command.as_str())).collect();
        assert_eq!(
            names,
            [
                ("mine", "harness", "own"),
                ("dup", "claude: local", "local-dup"),
                ("ok", "claude: project .mcp.json", "proj"),
                ("rea", "claude: user", "npx"),
                ("plugin_cd_chrome-devtools", "plugin cd", "npx"),
                ("plugin_uv_browser-use", "plugin uv", "uvx"),
                ("cx", "codex", "codex-srv"),
            ]
        );
        let get = |n: &str| found.iter().find(|s| s.name == n).unwrap().clone();
        assert_eq!(get("rea").env["K"], "dflt", "a default of an unset variable");
        assert_eq!(get("ok").args, [format!("{root_s}/x")]);
        assert_eq!(get("plugin_cd_chrome-devtools").args, [format!("{}/s.js", installed("cd").display())]);
        assert_eq!(get("plugin_uv_browser-use").args, [claude.join("plugins").join("data").join("uv-m").to_string_lossy().into_owned()]);
        assert_eq!(get("plugin_uv_browser-use").env["CLAUDE_PLUGIN_ROOT"], installed("uv").to_string_lossy());
        assert_eq!(get("cx").env["T"], "1");
    }

    #[test]
    fn places_follow_the_window_environment() {
        let env: BTreeMap<&str, &str> = [("HOME", "/h"), ("CLAUDE_CONFIG_DIR", "/p/claude"), ("MULTITOOL_USERS_CLAUDE_JSON", "/h/.claude.json")].into();
        let p = Places::from_env(&|n| env.get(n).map(|v| v.to_string()), "/app", BTreeMap::new());
        assert_eq!(p.claude_dir, Some(PathBuf::from("/p/claude")));
        assert_eq!(p.claude_jsons, [PathBuf::from("/h/.claude.json"), PathBuf::from("/p/claude/.claude.json")]);
        assert_eq!(p.codex_home, Some(PathBuf::from("/h/.codex")));
        let plain: BTreeMap<&str, &str> = [("HOME", "/h")].into();
        let p = Places::from_env(&|n| plain.get(n).map(|v| v.to_string()), "/app", BTreeMap::new());
        assert_eq!((p.claude_dir, p.claude_jsons), (Some(PathBuf::from("/h/.claude")), vec![PathBuf::from("/h/.claude.json")]));
    }

    #[test]
    fn names_and_variables() {
        assert_eq!(tool_safe("plugin:a b:c"), "plugin_a_b_c");
        let vars = |n: &str| (n == "A").then(|| "1".to_string());
        assert_eq!(expand("x${A}y${B}z${B:-2}${", &vars), "x1y${B}z2${");
    }
}
