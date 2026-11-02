//! Keeps a project's docs tree and its journal consistent (ADR 0017, CON-43..45): the structural
//! check, the hook that prompts the agent, and project initialization. The check looks at ids and
//! statuses, never at whether the prose agrees. All of it is advisory: it cannot veto anything.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use harness_api::event::HookOutcome;
use harness_api::platform::Fs;
use harness_api::*;

/// What `project_init` writes, with an example entry in each.
const TEMPLATES: [(&str, &str); 14] = [
    ("README.md", include_str!("../templates/README.md")),
    ("docs/architecture/overview.md", include_str!("../templates/docs/architecture/overview.md")),
    ("docs/spec/README.md", include_str!("../templates/docs/spec/README.md")),
    ("docs/spec/principles.md", include_str!("../templates/docs/spec/principles.md")),
    ("docs/spec/interfaces.md", include_str!("../templates/docs/spec/interfaces.md")),
    ("docs/spec/invariants.md", include_str!("../templates/docs/spec/invariants.md")),
    ("docs/spec/constraints.md", include_str!("../templates/docs/spec/constraints.md")),
    ("docs/spec/open-questions.md", include_str!("../templates/docs/spec/open-questions.md")),
    ("docs/spec/roadmap.md", include_str!("../templates/docs/spec/roadmap.md")),
    ("docs/decisions/README.md", include_str!("../templates/docs/decisions/README.md")),
    ("docs/decisions/0001-record-decisions.md", include_str!("../templates/docs/decisions/0001-record-decisions.md")),
    (".harness/README.md", include_str!("../templates/harness/README.md")),
    (".harness/policies/README.md", include_str!("../templates/harness/policies/README.md")),
    (".harness/debug/README.md", include_str!("../templates/harness/debug/README.md")),
];

const SPEC_FILES: [(&str, &str); 5] = [("P-", "principles"), ("IF-", "interfaces"), ("INV-", "invariants"), ("CON-", "constraints"), ("Q-", "open-questions")];
/// How many findings one reminder lists.
const REMINDER_LINES: usize = 10;
/// How many journal entries of each kind the check reads.
const SCAN: usize = 500;

/// A problem the check found.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding(pub String);

/// The doc ids a text mentions: `CON-12`, `IF-0.5`, `ADR 0016`.
pub fn ids_in(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut ids = Vec::new();
    let is_word = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    for start in 0..chars.len() {
        if start > 0 && is_word(chars[start - 1]) {
            continue;
        }
        let rest: String = chars[start..chars.len().min(start + 5)].iter().collect();
        if rest.starts_with("ADR ") || rest.starts_with("ADR-") {
            let digits: String = chars[start + 4..].iter().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() == 4 {
                ids.push(format!("ADR {digits}"));
            }
            continue;
        }
        let Some((prefix, _)) = SPEC_FILES.iter().find(|(p, _)| rest.starts_with(p)) else { continue };
        let mut end = start + prefix.len();
        let digits_from = end;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        if end == digits_from {
            continue;
        }
        if end + 1 < chars.len() && chars[end] == '.' && chars[end + 1].is_ascii_digit() {
            end += 1;
            while end < chars.len() && chars[end].is_ascii_digit() {
                end += 1;
            }
        }
        ids.push(chars[start..end].iter().collect());
    }
    ids
}

/// A project's docs tree, read through the platform.
pub struct Tree {
    /// (path relative to the project, text) of every Markdown file under `docs/`.
    files: Vec<(String, String)>,
    adrs: Vec<String>,
}

fn join(root: &str, path: &str) -> String {
    format!("{}/{path}", root.trim_end_matches('/'))
}

impl Tree {
    /// The tree, or `None` if the project has not been initialized (no mirror then, CON-44).
    pub async fn load(fs: &dyn Fs, root: &str) -> Option<Tree> {
        fs.list(&join(root, "docs/spec")).await.ok()?;
        let mut files = Vec::new();
        let mut pending = vec!["docs".to_string()];
        while let Some(dir) = pending.pop() {
            for name in fs.list(&join(root, &dir)).await.unwrap_or_default() {
                let path = format!("{dir}/{}", name.trim_end_matches('/'));
                if name.ends_with('/') {
                    pending.push(path);
                } else if name.ends_with(".md")
                    && let Ok(bytes) = fs.read(&join(root, &path)).await
                {
                    files.push((path, String::from_utf8_lossy(&bytes).into_owned()));
                }
            }
        }
        files.sort();
        let adrs = files.iter().filter_map(|(p, _)| p.strip_prefix("docs/decisions/")).filter(|n| n.as_bytes().first().is_some_and(u8::is_ascii_digit)).map(|n| n[..4.min(n.len())].to_string()).collect();
        Some(Tree { files, adrs })
    }

    fn text(&self, path: &str) -> &str {
        self.files.iter().find(|(p, _)| p == path).map_or("", |(_, t)| t)
    }

    /// The file that defines an id.
    fn home(id: &str) -> String {
        match SPEC_FILES.iter().find(|(p, _)| id.starts_with(p)) {
            Some((_, name)) => format!("docs/spec/{name}.md"),
            None => "docs/decisions/".to_string(),
        }
    }

    fn defines(&self, id: &str) -> bool {
        match id.strip_prefix("ADR ") {
            Some(number) => self.adrs.iter().any(|a| a == number),
            None => self.text(&Self::home(id)).contains(&format!("**{id}**")),
        }
    }

    /// The line of the file that defines this id.
    fn row(&self, id: &str) -> Option<&str> {
        self.text(&Self::home(id)).lines().find(|l| l.contains(&format!("**{id}**")))
    }

    /// What can be told from the files alone: this much runs in CI, with no journal.
    pub fn check_files(&self) -> Vec<Finding> {
        let mut found = BTreeSet::new();
        for (prefix, name) in SPEC_FILES {
            let path = format!("docs/spec/{name}.md");
            let mut seen = BTreeSet::new();
            for line in self.text(&path).lines().filter(|l| l.starts_with('|')) {
                let first = line.split('|').nth(1).unwrap_or_default();
                let Some(id) = ids_in(first).into_iter().find(|i| i.starts_with(prefix)) else { continue };
                if !seen.insert(id.clone()) {
                    found.insert(Finding(format!("{id} is defined twice in {path}")));
                }
            }
        }
        for (path, text) in &self.files {
            for id in ids_in(text) {
                if !self.defines(&id) {
                    found.insert(Finding(format!("{path} cites {id}, which {} does not define", Self::home(&id))));
                }
            }
        }
        let index = self.text("docs/decisions/README.md");
        for adr in &self.adrs {
            if !index.contains(&format!("({adr}-")) {
                found.insert(Finding(format!("ADR {adr} is missing from docs/decisions/README.md")));
            }
        }
        found.into_iter().collect()
    }

    /// The journal against the files.
    pub async fn check_journal(&self, memory: &dyn Memory) -> PluginResult<Vec<Finding>> {
        let list = |flags: &[&str], open: Option<bool>| JournalFilter { flags: flags.iter().map(|f| f.to_string()).collect(), open, limit: SCAN, ..Default::default() };
        let binding = memory.journal_list(list(&["specification", "constraint"], None)).await?;
        let open_questions = memory.journal_list(list(&["question"], Some(true))).await?;
        let closed_questions = memory.journal_list(list(&["question"], Some(false))).await?;
        let deprecated = memory.journal_list(list(&["deprecated"], None)).await?;
        let is_deprecated = |e: &JournalRecord| e.flags.iter().any(|f| f == "deprecated");

        let mut found = BTreeSet::new();
        let mut live = BTreeSet::new();
        for entry in binding.iter().filter(|e| !is_deprecated(e)) {
            let ids = ids_in(&entry.body);
            if ids.is_empty() {
                found.insert(Finding(format!("journal#{} binds but carries no doc id, so no file mirrors it", entry.id)));
            }
            for id in ids {
                if !self.defines(&id) {
                    found.insert(Finding(format!("journal#{} states {id}, which {} does not define", entry.id, Self::home(&id))));
                }
                live.insert(id);
            }
        }
        let struck = |id: &str, tree: &Tree| tree.row(id).is_some_and(|row| row.contains(&format!("~~**{id}**~~")));
        for entry in &open_questions {
            for id in ids_in(&entry.body).into_iter().filter(|i| i.starts_with("Q-")) {
                live.insert(id.clone());
                if struck(&id, self) {
                    found.insert(Finding(format!("journal#{} keeps {id} open, but open-questions.md has it settled", entry.id)));
                }
            }
        }
        for entry in &closed_questions {
            for id in ids_in(&entry.body).into_iter().filter(|i| i.starts_with("Q-")) {
                live.insert(id.clone());
                if self.defines(&id) && !struck(&id, self) {
                    found.insert(Finding(format!("journal#{} settled {id}, but open-questions.md does not strike it", entry.id)));
                }
            }
        }
        for entry in &deprecated {
            for id in ids_in(&entry.body) {
                let marked = self.row(&id).is_none_or(|row| row.contains("~~") || row.to_lowercase().contains("replaced") || row.to_lowercase().contains("deprecated"));
                if !live.contains(&id) && !marked {
                    found.insert(Finding(format!("journal#{} is deprecated and nothing else states {id}, but {} does not mark it replaced", entry.id, Self::home(&id))));
                }
            }
        }
        Ok(found.into_iter().collect())
    }
}

/// Creates what is missing of the docs tree and `.harness/`; never overwrites (CON-44).
pub async fn project_init(fs: &dyn Fs, root: &str) -> PluginResult<Vec<String>> {
    let mut created = Vec::new();
    for (path, text) in TEMPLATES {
        let full = join(root, path);
        if fs.read(&full).await.is_err() {
            fs.write(&full, text.as_bytes()).await?;
            created.push(path.to_string());
        }
    }
    Ok(created)
}

/// The file a memory write mirrors into, from what the tool was given. `None` for a write that
/// mirrors nowhere: an update, a finding, a lesson, a verification.
pub fn mirror_of(call: &ToolCall) -> Option<&'static str> {
    let flagged = |flag: &str| call.input["flags"].as_array().is_some_and(|f| f.iter().any(|v| v == flag));
    match call.name.as_str() {
        "journal_close" => Some("docs/spec/open-questions.md or docs/spec/roadmap.md, for a settled question or a finished todo"),
        "mindmap_new" | "mindmap_add" => Some("docs/architecture/overview.md, if this is the architecture map"),
        "flowchart_new" | "flowchart_add" => Some("the chart's page in docs/, its diagram and SVG"),
        "journal_add" if flagged("deprecated") => Some("the file that states it: update it, or mark the item replaced"),
        "journal_add" if flagged("specification") => Some("docs/spec/interfaces.md"),
        "journal_add" if flagged("constraint") => Some("docs/spec/constraints.md"),
        "journal_add" if flagged("question") => Some("docs/spec/open-questions.md"),
        "journal_add" if flagged("todo") => Some("docs/spec/roadmap.md"),
        "journal_add" => match call.input["category"].as_str() {
            Some("decision") => Some("its record in docs/decisions/ and the index"),
            Some("invariant") => Some("docs/spec/invariants.md"),
            Some("principle") => Some("docs/spec/principles.md"),
            Some("specification") => Some("docs/spec/interfaces.md"),
            Some("constraint") => Some("docs/spec/constraints.md"),
            _ => None,
        },
        _ => None,
    }
}

/// The built-in hook (CON-45): names the file after a memory write, and at the end of a turn adds
/// one reminder of what the check finds. It never vetoes, so it cannot stop or loop anything.
pub struct DocsMirrorHook {
    fs: Rc<dyn Fs>,
    memory: Rc<dyn Memory>,
    root: String,
    last: RefCell<Vec<Finding>>,
}

impl DocsMirrorHook {
    pub fn new(fs: Rc<dyn Fs>, memory: Rc<dyn Memory>, root: &str) -> Self {
        DocsMirrorHook { fs, memory, root: root.to_string(), last: RefCell::default() }
    }

    async fn at_stop(&self, tree: &Tree) -> HookOutcome {
        let mut found = tree.check_files();
        match tree.check_journal(self.memory.as_ref()).await {
            Ok(more) => found.extend(more),
            Err(e) => return HookOutcome::Failed { message: e.to_string() },
        }
        if found == *self.last.borrow() {
            return HookOutcome::Ok;
        }
        self.last.replace(found.clone());
        if found.is_empty() {
            return HookOutcome::Ok;
        }
        let mut lines = vec!["The documentation and the journal disagree; settle these before going on:".to_string()];
        lines.extend(found.iter().take(REMINDER_LINES).map(|f| format!("- {}", f.0)));
        if found.len() > REMINDER_LINES {
            lines.push(format!("- and {} more: multitool docs check", found.len() - REMINDER_LINES));
        }
        HookOutcome::Reminder { lines }
    }
}

impl Hook for DocsMirrorHook {
    fn name(&self) -> &str {
        "docs-mirror"
    }

    fn applies(&self, point: &HookPoint<'_>) -> bool {
        match point {
            HookPoint::Stop => true,
            HookPoint::PostToolUse { call, output } => !output.is_error && mirror_of(call).is_some(),
            _ => false,
        }
    }

    fn quiet_when_ok(&self) -> bool {
        true
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        Box::pin(async move {
            let Some(tree) = Tree::load(self.fs.as_ref(), &self.root).await else { return HookOutcome::Ok };
            match point {
                HookPoint::Stop => self.at_stop(&tree).await,
                HookPoint::PostToolUse { call, .. } => match mirror_of(call) {
                    Some(file) => HookOutcome::Reminder { lines: vec![format!("Mirror what you just wrote into {file}, in this same change, and state its doc id (P-, IF-, INV-, CON-, Q-, ADR) in the journal entry.")] },
                    None => HookOutcome::Ok,
                },
                _ => HookOutcome::Ok,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_memory_journal::plugin::JournalMemory;
    use serde_json::json;
    use std::path::Path;

    struct Project {
        dir: tempfile::TempDir,
        fs: Rc<dyn Fs>,
    }

    fn project() -> Project {
        Project { dir: tempfile::tempdir().unwrap(), fs: Rc::new(harness_platform_native::NativeFs) }
    }

    impl Project {
        fn root(&self) -> String {
            self.dir.path().to_string_lossy().into_owned()
        }
        fn init(&self) -> Vec<String> {
            pollster::block_on(project_init(self.fs.as_ref(), &self.root())).unwrap()
        }
        fn tree(&self) -> Tree {
            pollster::block_on(Tree::load(self.fs.as_ref(), &self.root())).expect("a tree")
        }
        fn append(&self, path: &str, text: &str) {
            let full = self.dir.path().join(path);
            let old = std::fs::read_to_string(&full).unwrap_or_default();
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, old + text).unwrap();
        }
    }

    fn journal(dir: &Path) -> JournalMemory {
        JournalMemory::open(dir, Path::new("k"), "t").unwrap()
    }

    fn add(m: &JournalMemory, body: &str, flags: &[&str]) -> JournalRecord {
        let entry = JournalEntry { blocked_by: None, body: body.into(), category: "decision".into(), tags: vec!["t".into()], flags: flags.iter().map(|f| f.to_string()).collect(), salience: None, feature: None };
        pollster::block_on(m.journal_add(entry)).unwrap()
    }

    fn messages(found: Vec<Finding>) -> String {
        found.into_iter().map(|f| f.0).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn ids_are_found_at_word_boundaries() {
        assert_eq!(ids_in("see CON-12, IF-0.5. INV-3 and (Q-7) in ADR 0016; ADR-0017"), ["CON-12", "IF-0.5", "INV-3", "Q-7", "ADR 0016", "ADR 0017"]);
        assert!(ids_in("HTTP-1 UTF-8 ACON-1 P-x ADR 12 pre-P-3").is_empty());
    }

    #[test]
    fn init_fills_only_what_is_missing_and_the_fresh_tree_passes() {
        let p = project();
        std::fs::write(p.dir.path().join("README.md"), "mine").unwrap();
        let created = p.init();
        assert!(!created.contains(&"README.md".to_string()) && created.contains(&"docs/spec/constraints.md".to_string()) && created.contains(&".harness/policies/README.md".to_string()), "{created:?}");
        assert_eq!(std::fs::read_to_string(p.dir.path().join("README.md")).unwrap(), "mine");
        p.append("docs/spec/roadmap.md", "edited");
        assert!(p.init().is_empty(), "a second run writes nothing");
        assert!(std::fs::read_to_string(p.dir.path().join("docs/spec/roadmap.md")).unwrap().ends_with("edited"));
        assert_eq!(messages(p.tree().check_files()), "");
    }

    #[test]
    fn the_files_alone_show_duplicates_dangling_ids_and_a_missing_index_line() {
        let p = project();
        p.init();
        p.append("docs/spec/constraints.md", "| **CON-1** | twice |\n");
        p.append("docs/spec/roadmap.md", "Follows CON-9 and ADR 0009.\n");
        p.append("docs/decisions/0002-other.md", "# 0002\n");
        let found = messages(p.tree().check_files());
        assert!(found.contains("CON-1 is defined twice"), "{found}");
        assert!(found.contains("cites CON-9") && found.contains("cites ADR 0009"), "{found}");
        assert!(found.contains("ADR 0002 is missing"), "{found}");
    }

    #[test]
    fn the_journal_is_held_to_the_files() {
        let p = project();
        p.init();
        let dir = tempfile::tempdir().unwrap();
        let m = journal(dir.path());
        let check = |t: &Tree| messages(pollster::block_on(t.check_journal(&m)).unwrap());
        assert_eq!(check(&p.tree()), "");

        let no_id = add(&m, "Prices are in cents.", &["specification"]);
        add(&m, "CON-1: the stable version.", &["constraint"]);
        let missing = add(&m, "CON-7: never on weekends.", &["constraint"]);
        add(&m, "CON-8: an old rule.", &["constraint", "deprecated"]);
        let old = add(&m, "INV-1 no longer holds.", &["deprecated"]);
        let open = add(&m, "Q-2: reopened?", &["question"]);
        let settled = add(&m, "Q-1: which storage?", &["question"]);
        pollster::block_on(m.journal_close(settled.id, "sqlite")).unwrap();
        let found = check(&p.tree());
        assert!(found.contains(&format!("journal#{} binds but carries no doc id", no_id.id)), "{found}");
        assert!(found.contains(&format!("journal#{} states CON-7", missing.id)), "{found}");
        assert!(!found.contains("CON-1") && !found.contains("CON-8 "), "{found}");
        assert!(found.contains(&format!("journal#{} is deprecated and nothing else states INV-1", old.id)), "{found}");
        assert!(found.contains(&format!("journal#{} keeps Q-2 open", open.id)), "{found}");
        assert!(found.contains(&format!("journal#{} settled Q-1", settled.id)), "{found}");
    }

    fn call(name: &str, input: serde_json::Value) -> ToolCall {
        ToolCall { id: "c".into(), name: name.into(), input }
    }

    #[test]
    fn each_write_names_its_file() {
        let add = |category: &str, flags: serde_json::Value| mirror_of(&call("journal_add", json!({"category": category, "flags": flags})));
        assert_eq!(add("decision", json!([])), Some("its record in docs/decisions/ and the index"));
        assert_eq!(add("x", json!(["constraint"])), Some("docs/spec/constraints.md"));
        assert_eq!(add("x", json!(["specification", "constraint"])), Some("docs/spec/interfaces.md"));
        assert_eq!(add("x", json!(["question"])), Some("docs/spec/open-questions.md"));
        assert_eq!(add("x", json!(["todo"])), Some("docs/spec/roadmap.md"));
        for silent in ["update", "finding", "lesson", "verification"] {
            assert_eq!(add(silent, json!([])), None, "{silent}");
        }
        assert!(mirror_of(&call("flowchart_add", json!({}))).unwrap().contains("chart"));
        assert_eq!(mirror_of(&call("journal_search", json!({}))), None);
        assert_eq!(mirror_of(&call("read", json!({}))), None);
    }

    #[test]
    fn the_hook_is_silent_without_a_tree_and_reminds_once() {
        let p = project();
        let dir = tempfile::tempdir().unwrap();
        let m = journal(dir.path());
        add(&m, "Prices are in cents.", &["specification"]);
        let shared: Rc<dyn Memory> = Rc::new(journal(dir.path()));
        let hook = DocsMirrorHook::new(p.fs.clone(), shared, &p.root());
        let written = call("journal_add", json!({"category": "decision"}));
        let output = ToolOutput::ok("x".to_string());
        let post = HookPoint::PostToolUse { call: &written, output: &output };
        assert_eq!(pollster::block_on(hook.run(&post)), HookOutcome::Ok, "no tree");
        assert_eq!(pollster::block_on(hook.run(&HookPoint::Stop)), HookOutcome::Ok, "no tree");

        p.init();
        assert!(matches!(pollster::block_on(hook.run(&post)), HookOutcome::Reminder { lines } if lines[0].contains("docs/decisions/")));
        let first = pollster::block_on(hook.run(&HookPoint::Stop));
        assert!(matches!(&first, HookOutcome::Reminder { lines } if lines.iter().any(|l| l.contains("carries no doc id"))), "{first:?}");
        assert_eq!(pollster::block_on(hook.run(&HookPoint::Stop)), HookOutcome::Ok, "nothing changed, no second reminder");
        let failed = ToolOutput::error("no".to_string());
        assert!(!hook.applies(&HookPoint::PostToolUse { call: &written, output: &failed }));
        assert!(!hook.applies(&HookPoint::SessionStart));
    }
}
