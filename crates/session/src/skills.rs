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

// --- Creating skills (CON-204, journal#1705) ---------------------------------------------------

/// Where a skill made by an agent goes: the harness's source, and the delegates' own folders.
#[derive(Debug, Clone, Default)]
pub struct SkillPlaces {
    pub root: PathBuf,
    /// The harness's user skills: `<harness home>/skills`.
    pub harness_skills: Option<PathBuf>,
    /// Claude Code's folder (its skills are `<it>/skills`).
    pub claude_dir: Option<PathBuf>,
    /// Codex's home (its skills are `<it>/skills`).
    pub codex_home: Option<PathBuf>,
}

/// The marker a delegate's copy carries: only a folder with it is the harness's to change.
pub const MARKER: &str = ".multitool-skill";

impl SkillPlaces {
    /// From a session's environment. A harness of its own (a test's data dir, `sandboxed`) keeps
    /// every user folder inside `own_home` and never writes to the user's.
    pub fn from_env(read: &dyn Fn(&str) -> Option<String>, root: &str, sandboxed: Option<&Path>) -> SkillPlaces {
        if let Some(home) = sandboxed {
            return SkillPlaces { root: root.into(), harness_skills: Some(home.join("skills")), claude_dir: None, codex_home: None };
        }
        let places = harness_tools_mcp::discovery::Places::from_env(read, root, Default::default());
        let set = |n: &str| read(n).filter(|v| !v.is_empty());
        let harness = set("MULTITOOL_HOME").map(PathBuf::from).or_else(|| set("HOME").or_else(|| set("USERPROFILE")).map(|h| PathBuf::from(h).join(".harness")));
        SkillPlaces { root: root.into(), harness_skills: harness.map(|h| h.join("skills")), claude_dir: places.claude_dir, codex_home: places.codex_home }
    }

    fn source(&self, scope: &str, name: &str) -> Result<PathBuf, String> {
        match scope {
            "project" => Ok(self.root.join(".harness").join("skills").join(name)),
            "user" => Ok(self.harness_skills.as_ref().ok_or("no home folder for the user's skills")?.join(name)),
            other => Err(format!("scope is project or user, not {other:?}")),
        }
    }

    /// Each delegate's native folder for the skill: (which program, the folder).
    fn mirrors(&self, scope: &str, name: &str) -> Vec<(&'static str, PathBuf)> {
        if scope == "project" {
            return vec![("Claude Code", self.root.join(".claude").join("skills").join(name)), ("Codex", self.root.join(".agents").join("skills").join(name))];
        }
        let mut out = Vec::new();
        if let Some(dir) = &self.claude_dir {
            out.push(("Claude Code", dir.join("skills").join(name)));
        }
        if let Some(dir) = &self.codex_home {
            out.push(("Codex", dir.join("skills").join(name)));
        }
        out
    }
}

/// Anthropic's Agent Skills rules for a name and a description
/// (platform.claude.com/docs/en/agents-and-tools/agent-skills/overview).
pub fn validate(name: &str, description: Option<&str>) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > 64 {
        return Err(format!("a skill's name has 1 to 64 characters: {name:?} has {}", name.chars().count()));
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(format!("a skill's name has only lowercase letters, digits and hyphens: {name:?}"));
    }
    if name.contains("anthropic") || name.contains("claude") {
        return Err(format!("a skill's name may not contain 'anthropic' or 'claude': {name:?}"));
    }
    if let Some(d) = description {
        if d.trim().is_empty() {
            return Err("a skill's description is required: say what it does and when to use it".into());
        }
        if d.chars().count() > 1024 {
            return Err(format!("a skill's description has at most 1,024 characters: this one has {}", d.chars().count()));
        }
        if has_xml_tag(d) {
            return Err("a skill's description has no XML tags".into());
        }
    }
    Ok(())
}

fn has_xml_tag(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.iter().enumerate().any(|(i, b)| *b == b'<' && bytes.get(i + 1).is_some_and(|c| c.is_ascii_alphabetic() || *c == b'/'))
}

fn skill_md(name: &str, description: &str, body: &str) -> String {
    let description = description.replace('\n', " ");
    format!("---\nname: {name}\ndescription: {description}\n---\n\n{}\n", body.trim_end())
}

/// A relative file path inside a skill's folder: no absolute path, no '..', not the marker.
fn safe_relative(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        return Err(format!("a skill's file is a path inside its folder: {path:?}"));
    }
    if path == MARKER || path == "SKILL.md" {
        return Err(format!("{path} is not a file to set: SKILL.md comes from description and body"));
    }
    Ok(p.to_path_buf())
}

/// Every file of a folder, by its relative path, the marker left out.
fn tree(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                todo.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if rel != MARKER {
                    out.insert(rel, std::fs::read(&path).unwrap_or_default());
                }
            }
        }
    }
    out
}

fn digest(files: &std::collections::BTreeMap<String, Vec<u8>>) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for (path, bytes) in files {
        h.update(path.as_bytes());
        h.update([0]);
        h.update(bytes);
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn is_ours(dir: &Path) -> bool {
    dir.join(MARKER).is_file()
}

/// The source copied to a delegate's folder, with the marker. Answers what happened there.
fn mirror_to(source: &Path, label: &str, target: &Path) -> String {
    if target.exists() && !is_ours(target) {
        return format!("{label}: a skill of that name of its own is at {}, left as it is", target.display());
    }
    let files = tree(source);
    if target.exists() {
        let _ = std::fs::remove_dir_all(target);
    }
    for (rel, bytes) in &files {
        let path = target.join(rel);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&path, bytes) {
            return format!("{label}: could not write {}: {e}", path.display());
        }
    }
    let marker = json!({"source": source.to_string_lossy(), "sha256": digest(&files), "by": "multitool (CON-204)"});
    let _ = std::fs::write(target.join(MARKER), marker.to_string());
    format!("{label}: {}", target.display())
}

/// skill_create, skill_edit, skill_delete (CON-204).
pub struct SkillWrite {
    pub name: &'static str,
    pub places: std::sync::Arc<SkillPlaces>,
}

pub const WRITE_TOOLS: [&str; 3] = ["skill_create", "skill_edit", "skill_delete"];

/// What a call names: the skill, its scope, its source folder and the delegates' folders.
struct Targets {
    name: String,
    scope: String,
    source: PathBuf,
    mirrors: Vec<(&'static str, PathBuf)>,
}

impl SkillWrite {
    fn targets(&self, input: &Value) -> Result<Targets, String> {
        let name = input["name"].as_str().map(str::trim).unwrap_or_default().to_string();
        validate(&name, None)?;
        let scope = input["scope"].as_str().unwrap_or("project").to_string();
        let source = self.places.source(&scope, &name)?;
        let mirrors = self.places.mirrors(&scope, &name);
        Ok(Targets { name, scope, source, mirrors })
    }

    fn run(&self, input: &Value) -> Result<String, String> {
        let Targets { name, scope, source, mirrors } = self.targets(input)?;
        let files = input["files"].as_object().cloned().unwrap_or_default();
        let mut changed_files = Vec::new();
        for (path, content) in &files {
            changed_files.push((safe_relative(path)?, content.as_str().map(str::to_string)));
        }
        let mut lines = Vec::new();
        match self.name {
            "skill_create" => {
                let description = input["description"].as_str().unwrap_or_default();
                validate(&name, Some(description))?;
                if source.join("SKILL.md").exists() {
                    return Err(format!("the {scope} skill {name} exists already: change it with skill_edit"));
                }
                std::fs::create_dir_all(&source).map_err(|e| format!("{}: {e}", source.display()))?;
                std::fs::write(source.join("SKILL.md"), skill_md(&name, description, input["body"].as_str().unwrap_or_default())).map_err(|e| e.to_string())?;
                for (rel, content) in &changed_files {
                    if let Some(text) = content {
                        let path = source.join(rel);
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
                    }
                }
                lines.push(format!("Created the {scope} skill {name} at {}.", source.display()));
            }
            "skill_edit" => {
                let text = std::fs::read_to_string(source.join("SKILL.md")).map_err(|_| format!("there is no {scope} skill {name} made by the harness at {}", source.display()))?;
                let (_, old_description, old_body) = parse(&text);
                let description = input["description"].as_str().map(str::to_string).or(old_description).unwrap_or_default();
                validate(&name, Some(&description))?;
                let body = input["body"].as_str().map(str::to_string).unwrap_or_else(|| old_body.trim().to_string());
                std::fs::write(source.join("SKILL.md"), skill_md(&name, &description, &body)).map_err(|e| e.to_string())?;
                for (rel, content) in &changed_files {
                    let path = source.join(rel);
                    match content {
                        Some(text) => {
                            if let Some(parent) = path.parent() {
                                let _ = std::fs::create_dir_all(parent);
                            }
                            std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
                        }
                        None => {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
                lines.push(format!("Changed the {scope} skill {name} at {}.", source.display()));
            }
            _ => {
                if !source.join("SKILL.md").exists() {
                    return Err(format!("there is no {scope} skill {name} made by the harness at {}", source.display()));
                }
                std::fs::remove_dir_all(&source).map_err(|e| format!("{}: {e}", source.display()))?;
                lines.push(format!("Removed the {scope} skill {name} from {}.", source.display()));
                for (label, target) in &mirrors {
                    if !target.exists() {
                        continue;
                    }
                    if is_ours(target) {
                        let _ = std::fs::remove_dir_all(target);
                        lines.push(format!("{label}: removed {}", target.display()));
                    } else {
                        lines.push(format!("{label}: a skill of that name of its own is at {}, left as it is", target.display()));
                    }
                }
                return Ok(lines.join("\n"));
            }
        }
        lines.push("Native copies for the delegates:".into());
        for (label, target) in &mirrors {
            lines.push(format!("- {}", mirror_to(&source, label, target)));
        }
        Ok(lines.join("\n"))
    }
}

impl Tool for SkillWrite {
    fn spec(&self) -> ToolSpec {
        let scope = json!({"type": "string", "enum": ["project", "user"], "description": "project: this project's .harness/skills; user: your ~/.harness/skills, for every project."});
        let (description, schema) = match self.name {
            "skill_create" => (
                "Make a skill (CON-204): instructions an agent loads when a task matches its description. Its source goes in the harness's skills (.harness/skills of the project, or the user's ~/.harness/skills, by scope), and a native copy goes to Claude Code's and Codex's skill folders so each finds it as its own. name: lowercase letters, digits and hyphens, at most 64, without 'anthropic' or 'claude'. description: what it does and when to use it, at most 1,024 characters. Asks the user, listing every file it writes.",
                json!({"type": "object", "properties": {"name": {"type": "string"}, "description": {"type": "string"}, "body": {"type": "string", "description": "The instructions, in Markdown."}, "scope": scope, "files": {"type": "object", "description": "Other files of the skill, by path inside its folder, e.g. scripts/run.sh.", "additionalProperties": {"type": "string"}}}, "required": ["name", "description", "body", "scope"]}),
            ),
            "skill_edit" => (
                "Change a skill made with skill_create: its description, its body, or its files (a file set to null is removed). The change reaches the source and every native copy the harness made; a delegate's own skill of that name is never touched. Asks the user.",
                json!({"type": "object", "properties": {"name": {"type": "string"}, "scope": scope, "description": {"type": "string"}, "body": {"type": "string"}, "files": {"type": "object", "additionalProperties": {"type": ["string", "null"]}}}, "required": ["name", "scope"]}),
            ),
            _ => (
                "Remove a skill made with skill_create: its source and the native copies the harness made for the delegates. A delegate's own skill of that name stays. Asks the user.",
                json!({"type": "object", "properties": {"name": {"type": "string"}, "scope": scope}, "required": ["name", "scope"]}),
            ),
        };
        ToolSpec { name: self.name.into(), description: description.into(), schema }
    }

    /// Every path it writes, so the user sees them all when asked.
    fn capabilities(&self, input: &Value) -> Result<Vec<Capability>, String> {
        let Targets { source, mirrors, .. } = self.targets(input)?;
        let mut paths = vec![source.join("SKILL.md")];
        for path in input["files"].as_object().into_iter().flat_map(|m| m.keys()) {
            paths.push(source.join(safe_relative(path)?));
        }
        for (_, target) in &mirrors {
            paths.push(target.clone());
        }
        if self.name == "skill_delete" {
            paths[0] = source;
        }
        Ok(paths.into_iter().map(|p| Capability::WritePath(p.to_string_lossy().into_owned())).collect())
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        let answer = match self.run(input) {
            Ok(text) => ToolOutput::ok(text),
            Err(e) => ToolOutput::error(e),
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

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mt-skillw-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn places(t: &Path) -> std::sync::Arc<SkillPlaces> {
        std::sync::Arc::new(SkillPlaces { root: t.join("p"), harness_skills: Some(t.join("harness/skills")), claude_dir: Some(t.join("claude")), codex_home: Some(t.join("codex")) })
    }

    fn call(places: &std::sync::Arc<SkillPlaces>, tool: &'static str, input: Value) -> Result<String, String> {
        SkillWrite { name: tool, places: places.clone() }.run(&input)
    }

    /// CON-204: a project skill's source, and a marked native copy for Claude Code and for Codex.
    #[test]
    fn a_project_skill_is_made_with_native_copies_and_kept_in_step() {
        let t = temp("project");
        let p = places(&t);
        let said = call(&p, "skill_create", json!({"name": "deploy-app", "description": "Ship the app the project's way", "body": "# Deploy\nRun ./ship.", "scope": "project", "files": {"scripts/ship.sh": "echo ship"}})).unwrap();
        let root = t.join("p");
        for dir in [".harness/skills/deploy-app", ".claude/skills/deploy-app", ".agents/skills/deploy-app"] {
            let md = std::fs::read_to_string(root.join(dir).join("SKILL.md")).unwrap();
            assert!(md.starts_with("---\nname: deploy-app\ndescription: Ship the app the project's way\n---"), "{dir}: {md}");
            assert_eq!(std::fs::read_to_string(root.join(dir).join("scripts/ship.sh")).unwrap(), "echo ship");
        }
        assert!(!root.join(".harness/skills/deploy-app").join(MARKER).exists(), "the source is no copy");
        let marker: Value = serde_json::from_str(&std::fs::read_to_string(root.join(".claude/skills/deploy-app").join(MARKER)).unwrap()).unwrap();
        assert!(marker["source"].as_str().unwrap().replace('\\', "/").ends_with(".harness/skills/deploy-app") && marker["sha256"].as_str().unwrap().len() == 64);
        assert!(said.contains("Claude Code:") && said.contains("Codex:"), "{said}");
        let found = skills(&root.to_string_lossy(), None);
        assert_eq!(found.iter().map(|s| (s.name.as_str(), s.from.as_str())).collect::<Vec<_>>(), [("deploy-app", "project")], "skill_list reads it");
        // An edit reaches the source and every copy; a file set to null goes.
        call(&p, "skill_edit", json!({"name": "deploy-app", "scope": "project", "body": "Run ./ship --prod.", "files": {"scripts/ship.sh": null, "notes.md": "n"}})).unwrap();
        for dir in [".harness/skills/deploy-app", ".claude/skills/deploy-app", ".agents/skills/deploy-app"] {
            let md = std::fs::read_to_string(root.join(dir).join("SKILL.md")).unwrap();
            assert!(md.contains("Run ./ship --prod.") && md.contains("description: Ship the app the project's way"), "{dir}: {md}");
            assert!(!root.join(dir).join("scripts/ship.sh").exists() && root.join(dir).join("notes.md").exists(), "{dir}");
        }
        assert!(call(&p, "skill_create", json!({"name": "deploy-app", "description": "again", "body": "", "scope": "project"})).unwrap_err().contains("skill_edit"));
        // A delete removes the source and the copies.
        call(&p, "skill_delete", json!({"name": "deploy-app", "scope": "project"})).unwrap();
        for dir in [".harness/skills/deploy-app", ".claude/skills/deploy-app", ".agents/skills/deploy-app"] {
            assert!(!root.join(dir).exists(), "{dir} removed");
        }
        let _ = std::fs::remove_dir_all(&t);
    }

    /// A user skill goes to the harness home, Claude Code's folder and Codex's home; a native skill
    /// of the same name that is not the harness's is never touched.
    #[test]
    fn a_user_skill_goes_to_the_users_folders_and_leaves_native_ones_alone() {
        let t = temp("user");
        let p = places(&t);
        let theirs = t.join("codex/skills/review");
        std::fs::create_dir_all(&theirs).unwrap();
        std::fs::write(theirs.join("SKILL.md"), "---\nname: review\ndescription: the user's own\n---\nMine.\n").unwrap();
        let said = call(&p, "skill_create", json!({"name": "review", "description": "Review a change", "body": "Look closely.", "scope": "user"})).unwrap();
        assert!(t.join("harness/skills/review/SKILL.md").exists() && t.join("claude/skills/review").join(MARKER).exists());
        assert!(said.contains("left as it is"), "{said}");
        assert_eq!(std::fs::read_to_string(theirs.join("SKILL.md")).unwrap(), "---\nname: review\ndescription: the user's own\n---\nMine.\n");
        call(&p, "skill_edit", json!({"name": "review", "scope": "user", "description": "Review a change, closely"})).unwrap();
        assert!(std::fs::read_to_string(theirs.join("SKILL.md")).unwrap().contains("the user's own"), "untouched by an edit");
        call(&p, "skill_delete", json!({"name": "review", "scope": "user"})).unwrap();
        assert!(theirs.join("SKILL.md").exists() && !t.join("claude/skills/review").exists() && !t.join("harness/skills/review").exists());
        let _ = std::fs::remove_dir_all(&t);
    }

    /// Anthropic's rules for a name and a description, and the paths a call asks for.
    #[test]
    fn names_and_descriptions_keep_the_rules_and_every_path_is_asked() {
        for bad in ["My Skill", "claude-helper", "my-anthropic", &"a".repeat(65), "<x>", ""] {
            assert!(validate(bad, None).is_err(), "{bad:?}");
        }
        assert!(validate("deploy-2", Some("Ships it")).is_ok());
        assert!(validate("x", Some(" ")).is_err());
        assert!(validate("x", Some(&"d".repeat(1025))).unwrap_err().contains("1,024"));
        assert!(validate("x", Some("use <b>this</b>")).is_err());
        assert!(validate("x", Some("a < b is fine")).is_ok());
        let t = temp("caps");
        let p = places(&t);
        let caps = SkillWrite { name: "skill_create", places: p.clone() }.capabilities(&json!({"name": "x", "scope": "project", "files": {"a/b.txt": "1"}})).unwrap();
        let paths: Vec<String> = caps.iter().map(|c| match c { Capability::WritePath(p) => p.replace('\\', "/"), other => panic!("{other:?}") }).collect();
        assert_eq!(paths.len(), 4, "{paths:?}");
        assert!(paths[0].ends_with(".harness/skills/x/SKILL.md") && paths[1].ends_with(".harness/skills/x/a/b.txt") && paths[2].ends_with(".claude/skills/x") && paths[3].ends_with(".agents/skills/x"), "{paths:?}");
        assert!(SkillWrite { name: "skill_create", places: p }.capabilities(&json!({"name": "x", "scope": "project", "files": {"../out": "1"}})).is_err(), "no path outside its folder");
        let _ = std::fs::remove_dir_all(&t);
    }
}
