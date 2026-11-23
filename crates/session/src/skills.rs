//! The harness's own skills (ADR 0030, journal#888): laid out as Claude Code lays out its own, with
//! `.harness` in place of `.claude`, the same on every adapter. `<project>/.harness/skills/<folder>/SKILL.md`
//! and the user's `~/.harness/skills/<folder>/SKILL.md` (MULTITOOL_HOME when set); the project's win on
//! the same name. Two tools serve them: `skill_list` (names and descriptions only, so a long list costs
//! little) and `skill_load` (one skill's instructions, and the paths of its files).
//!
//! SKILL.md starts with YAML frontmatter between `---` lines; `name` and `description` are read from
//! it (Anthropic's Agent Skills format, which Claude Code reads). Without a name the folder's is used;
//! without a description, the first line of the body.
//!
//! On the own loop they also find the skills of Claude Code and Codex (CON-201): Claude Code's own
//! (`<its folder>/skills`), the project's `.claude/skills`, its enabled plugins' (`<plugin>:<skill>`,
//! as Claude Code names them) and Codex's (`<CODEX_HOME>/skills`). The harness's own win a clash.

use std::path::{Path, PathBuf};

use harness_api::*;
use serde_json::{Value, json};

/// One skill found.
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// "project" or "user"; for another program's, where: "claude", "claude project", "plugin <name>", "codex".
    pub from: String,
    pub folder: PathBuf,
}

/// Where the user's own skills are: MULTITOOL_HOME's, else `~/.harness`'s.
pub fn users_skills_dir() -> Option<PathBuf> {
    match std::env::var_os("MULTITOOL_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join("skills")),
        None => std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".harness").join("skills")),
    }
}

/// The frontmatter's `name` and `description`, and the body after it.
fn parse(text: &str) -> (Option<String>, Option<String>, &str) {
    let Some(rest) = text.strip_prefix("---").map(|r| r.trim_start_matches('\r').strip_prefix('\n').unwrap_or(r)) else { return (None, None, text) };
    let Some(end) = rest.find("\n---") else { return (None, None, text) };
    let (front, after) = (&rest[..end], &rest[end + 4..]);
    let body = after.split_once('\n').map_or("", |(_, b)| b);
    let field = |key: &str| {
        front.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            (k.trim() == key).then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_string()).filter(|v| !v.is_empty())
        })
    };
    (field("name"), field("description"), body)
}

fn read_folder(dir: &Path, from: &str) -> Vec<Skill> {
    read_named(dir, from, "")
}

/// A folder's skills, each name after `prefix`.
fn read_named(dir: &Path, from: &str, prefix: &str) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let folder = entry.path();
        let Ok(text) = std::fs::read_to_string(folder.join("SKILL.md")) else { continue };
        let (name, description, body) = parse(&text);
        let name = format!("{prefix}{}", name.unwrap_or_else(|| entry.file_name().to_string_lossy().into_owned()));
        let description = description.or_else(|| body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_string)).unwrap_or_default();
        out.push(Skill { name, description, from: from.to_string(), folder });
    }
    out
}

/// The skills here: the project's, then the user's not shadowed by a project's of the same name.
pub fn skills(root: &str, users: Option<&Path>) -> Vec<Skill> {
    let mut found = read_folder(&Path::new(root).join(".harness").join("skills"), "project");
    for skill in users.map(|u| read_folder(u, "user")).unwrap_or_default() {
        if !found.iter().any(|s| s.name == skill.name) {
            found.push(skill);
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The skills of Claude Code and Codex for the project (CON-201), by precedence: Claude Code's
/// project's, its own, its enabled plugins', then Codex's.
pub fn agents_skills(places: &harness_tools_mcp::discovery::Places) -> Vec<Skill> {
    let mut found = read_folder(&Path::new(&places.root).join(".claude").join("skills"), "claude project");
    if let Some(dir) = &places.claude_dir {
        found.extend(read_folder(&dir.join("skills"), "claude"));
    }
    for plugin in harness_tools_mcp::discovery::enabled_plugins(places) {
        found.extend(read_named(&plugin.root.join("skills"), &format!("plugin {}", plugin.name), &format!("{}:", plugin.name)));
    }
    if let Some(codex) = &places.codex_home {
        found.extend(read_folder(&codex.join("skills"), "codex"));
    }
    found
}

/// The harness's skills, and after them those of `agents` not shadowed by a name of its own.
fn all_skills(root: &str, agents: Option<&harness_tools_mcp::discovery::Places>) -> Vec<Skill> {
    let mut found = skills(root, users_skills_dir().as_deref());
    for skill in agents.map(agents_skills).unwrap_or_default() {
        if !found.iter().any(|s| s.name == skill.name) {
            found.push(skill);
        }
    }
    found
}

/// The files of a skill's folder besides SKILL.md, two levels deep, by their path.
fn files_of(folder: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut todo = vec![(folder.to_path_buf(), 0)];
    while let Some((dir, depth)) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 2 {
                todo.push((path, depth + 1));
            } else if path.is_file() && path.file_name().is_some_and(|n| n != "SKILL.md") {
                out.push(path.to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

/// `skill_list`; with `agents`, on the own loop, Claude Code's and Codex's skills too (CON-201).
#[derive(Default)]
pub struct SkillList {
    pub agents: Option<std::sync::Arc<harness_tools_mcp::discovery::Places>>,
}
/// `skill_load`, of the same skills as [`SkillList`].
#[derive(Default)]
pub struct SkillLoad {
    pub agents: Option<std::sync::Arc<harness_tools_mcp::discovery::Places>>,
}

impl Tool for SkillList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_list".into(),
            description: if self.agents.is_some() {
                "The skills here, each by name and description: the harness's (the project's .harness/skills, the user's ~/.harness/skills), then Claude Code's and Codex's. When a task matches one's description, load it with skill_load before you start.".into()
            } else {
                "The harness's skills here: the project's (.harness/skills) and the user's (~/.harness/skills), each by name and description. When a task matches one's description, load it with skill_load before you start.".into()
            },
            schema: json!({"type": "object", "properties": {}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, _input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let list: Vec<Value> = all_skills(&cx.root, self.agents.as_deref()).iter().map(|s| json!({"name": s.name, "description": s.description, "from": s.from})).collect();
        let text = if list.is_empty() { "No skills here: none in .harness/skills of the project or of the user.".to_string() } else { json!({"skills": list}).to_string() };
        Box::pin(std::future::ready(ToolOutput::ok(text)))
    }
}

impl Tool for SkillLoad {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill_load".into(),
            description: "Load one of the skills skill_list names, by its name: its instructions, to follow, and the paths of the files that come with it.".into(),
            schema: json!({"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(vec![])
    }

    fn execute<'a>(&'a self, input: &'a Value, cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let name = input["name"].as_str().unwrap_or_default().trim();
        let found = all_skills(&cx.root, self.agents.as_deref());
        let answer = match found.iter().find(|s| s.name == name) {
            None => ToolOutput::error(format!("no skill {name:?}; there are: {}", found.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "))),
            Some(skill) => match std::fs::read_to_string(skill.folder.join("SKILL.md")) {
                Err(e) => ToolOutput::error(format!("reading {}: {e}", skill.folder.join("SKILL.md").display())),
                Ok(text) => {
                    let body = parse(&text).2.trim().to_string();
                    let files = files_of(&skill.folder);
                    let mut out = format!("Skill {} ({}), from {}:\n\n{body}", skill.name, skill.description, skill.folder.display());
                    if !files.is_empty() {
                        out.push_str(&format!("\n\nIts files:\n{}", files.join("\n")));
                    }
                    ToolOutput::ok(out)
                }
            },
        };
        Box::pin(std::future::ready(answer))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, folder: &str, text: &str) {
        std::fs::create_dir_all(dir.join(folder)).unwrap();
        std::fs::write(dir.join(folder).join("SKILL.md"), text).unwrap();
    }

    /// journal#888: a project skill and a user skill of the same name, the project's wins; a
    /// user-only skill is offered; the load answers the body and the files.
    #[test]
    fn the_projects_skill_wins_and_the_users_are_offered() {
        let root = std::env::temp_dir().join(format!("mt-skills-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let project = root.join("p");
        let users = root.join("u");
        write(&project.join(".harness/skills"), "deploy", "---\nname: deploy\ndescription: \"Ship the project's way\"\n---\n# Deploy\nRun ./ship.\n");
        std::fs::write(project.join(".harness/skills/deploy/ship.sh"), "x").unwrap();
        write(&users, "deploy", "---\nname: deploy\ndescription: the user's way\n---\nOther.\n");
        write(&users, "review", "---\nname: review\ndescription: Review a change\n---\nLook.\n");
        write(&users, "bare", "No frontmatter: the first line describes it.\n");
        let found = skills(&project.to_string_lossy(), Some(&users));
        let named: Vec<(&str, &str, &str)> = found.iter().map(|s| (s.name.as_str(), s.description.as_str(), s.from.as_str())).collect();
        assert_eq!(named, [("bare", "No frontmatter: the first line describes it.", "user"), ("deploy", "Ship the project's way", "project"), ("review", "Review a change", "user")]);
        let (_, _, body) = parse("---\nname: x\n---\nbody here\n");
        assert_eq!(body, "body here\n");
        let deploy = project.join(".harness/skills/deploy");
        assert_eq!(files_of(&deploy), [deploy.join("ship.sh").to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CON-201: Claude Code's skills (its own, the project's, an enabled plugin's) and Codex's are
    /// found after the harness's, which win a clash.
    #[test]
    fn claude_codes_and_codexs_skills_come_after_the_harnesss() {
        let t = std::env::temp_dir().join(format!("mt-agent-skills-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let (root, claude, codex) = (t.join("p"), t.join("claude"), t.join("codex"));
        write(&root.join(".harness/skills"), "review", "---\nname: review\ndescription: the harness's\n---\nOurs.\n");
        write(&root.join(".claude/skills"), "review", "---\nname: review\ndescription: Claude's project's\n---\nTheirs.\n");
        write(&root.join(".claude/skills"), "ship", "---\nname: ship\ndescription: Ship it\n---\nGo.\n");
        write(&claude.join("skills"), "rea", "---\nname: reverse-engineer-anything\ndescription: Reverse engineer\n---\nUse REA.\n");
        let plugin = claude.join("plugins/cache/m/cd/1");
        write(&plugin.join("skills"), "a11y", "---\nname: a11y-debugging\ndescription: Accessibility\n---\nA.\n");
        std::fs::write(claude.join("settings.json"), r#"{"enabledPlugins": {"cd@m": true}}"#).unwrap();
        std::fs::write(claude.join("plugins/installed_plugins.json"), serde_json::json!({"plugins": {"cd@m": [{"scope": "user", "installPath": plugin}]}}).to_string()).unwrap();
        write(&codex.join("skills"), "lcp", "---\nname: debug-optimize-lcp\ndescription: LCP\n---\nL.\n");
        let places = harness_tools_mcp::discovery::Places { root: root.to_string_lossy().into_owned(), claude_dir: Some(claude), codex_home: Some(codex), ..Default::default() };
        let found = all_skills(&places.root, Some(&places));
        let named: Vec<(&str, &str)> = found.iter().map(|s| (s.name.as_str(), s.from.as_str())).collect();
        assert!(named.contains(&("review", "project")) && !named.contains(&("review", "claude project")), "the harness's wins: {named:?}");
        for want in [("ship", "claude project"), ("reverse-engineer-anything", "claude"), ("cd:a11y-debugging", "plugin cd"), ("debug-optimize-lcp", "codex")] {
            assert!(named.contains(&want), "{want:?} in {named:?}");
        }
        assert!(all_skills(&places.root, None).iter().all(|s| s.from == "project" || s.from == "user"), "without agents, the harness's only");
        let _ = std::fs::remove_dir_all(&t);
    }
}
