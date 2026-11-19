//! Custom callbacks as project handlers (CON-183): `.harness/callbacks/<name>/` holds a
//! `config.toml` and an executable handler of the project's own. The daemon runs a handler only
//! once it is approved, pinned by its SHA-256, and only for the conversations subscribed to it.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::Digest;

/// What `config.toml` says, with the folder it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The folder's name, which is the callback's name.
    pub name: String,
    pub description: String,
    /// The callback types it serves; its name when the file names none.
    pub types: Vec<String>,
    pub args: Vec<String>,
    pub timeout_s: u64,
    /// The handler's path, absolute.
    pub handler: PathBuf,
    pub folder: PathBuf,
    /// What is wrong with it, when it cannot run: a bad file, a missing handler.
    pub problem: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    types: Vec<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    timeout_s: Option<u64>,
    #[serde(default)]
    handler: Option<String>,
}

pub const DEFAULT_TIMEOUT_S: u64 = 30;
const MAX_TIMEOUT_S: u64 = 3600;

/// The project's custom callbacks, by folder name.
pub fn found(root: &str) -> Vec<Found> {
    let base = Path::new(root).join(".harness").join("callbacks");
    let Ok(dirs) = std::fs::read_dir(&base) else { return Vec::new() };
    let mut out: Vec<Found> = dirs.flatten().filter(|d| d.path().is_dir()).filter_map(|d| read_one(&d.path())).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn read_one(folder: &Path) -> Option<Found> {
    let name = folder.file_name()?.to_string_lossy().into_owned();
    let mut problem = None;
    let config = match std::fs::read_to_string(folder.join("config.toml")) {
        Ok(text) => toml::from_str::<ConfigFile>(&text).unwrap_or_else(|e| {
            problem = Some(format!("config.toml is not valid: {}", e.message()));
            ConfigFile::default()
        }),
        Err(_) => {
            problem = Some("no config.toml".to_string());
            ConfigFile::default()
        }
    };
    // A handler is a Rust program built in its folder (journal#1373): target/release/<name>.
    let handler = folder.join(config.handler.clone().unwrap_or_else(|| format!("target/release/{name}")));
    if problem.is_none() && !is_executable(&handler) {
        problem = Some(format!("{} is not an executable file", handler.display()));
    }
    if problem.is_none() && is_script(&handler) {
        problem = Some(SCRIPT_REFUSED.to_string());
    }
    let types = if config.types.is_empty() { vec![config.name.clone().unwrap_or_else(|| name.clone())] } else { config.types };
    Some(Found { name, description: config.description, types, args: config.args, timeout_s: config.timeout_s.unwrap_or(DEFAULT_TIMEOUT_S).clamp(1, MAX_TIMEOUT_S), handler, folder: folder.to_path_buf(), problem })
}

/// Why a script is not run (journal#1373): a handler is a compiled Rust program.
pub const SCRIPT_REFUSED: &str = "a handler is a compiled Rust program (CON-183), not a script: build it with multitool callback build";

/// A file that starts with `#!` is a script for an interpreter.
fn is_script(path: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 2];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok() && &head == b"#!"
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// The callback of the project that serves `kind`.
pub fn serving(root: &str, kind: &str) -> Option<Found> {
    found(root).into_iter().find(|f| f.types.iter().any(|t| t == kind))
}

/// The handler file's SHA-256, hex.
pub fn sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// Where a handler stands with the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    /// Never approved: it does not run.
    Unapproved,
    Approved,
    /// Approved once, but the file has changed since: it does not run until approved again.
    Changed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct Approved {
    path: String,
    sha256: String,
}

/// The approved handlers, by project and name (daemon/callback_handlers.json).
pub struct Trust {
    path: PathBuf,
    map: BTreeMap<String, BTreeMap<String, Approved>>,
}

impl Trust {
    pub fn load(dir: &Path) -> Trust {
        let path = dir.join("callback_handlers.json");
        let map = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Trust { path, map }
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.map) {
            let _ = super::write_private(&self.path, &text);
        }
    }

    pub fn standing(&self, root: &str, found: &Found) -> Standing {
        match self.map.get(root).and_then(|m| m.get(&found.name)) {
            None => Standing::Unapproved,
            Some(a) if a.path == found.handler.to_string_lossy() && sha256(&found.handler).as_deref() == Some(a.sha256.as_str()) => Standing::Approved,
            Some(_) => Standing::Changed,
        }
    }

    /// Approves the handler as it is now.
    pub fn approve(&mut self, root: &str, found: &Found) -> Result<(), String> {
        if let Some(problem) = &found.problem {
            return Err(problem.clone());
        }
        let sha = sha256(&found.handler).ok_or_else(|| format!("{} cannot be read", found.handler.display()))?;
        self.map.entry(root.to_string()).or_default().insert(found.name.clone(), Approved { path: found.handler.to_string_lossy().into_owned(), sha256: sha });
        self.save();
        Ok(())
    }
}

/// A custom callback registered for a conversation (journal#1372): what the daemon runs when one of
/// its types comes to that conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// Always true here: a type the table holds is a custom one; the standard types are native.
    pub custom: bool,
    pub name: String,
    pub root: String,
    pub handler: PathBuf,
    pub args: Vec<String>,
    pub timeout_s: u64,
    /// The callback's folder, where its handler runs.
    #[serde(default)]
    pub folder: PathBuf,
}

/// Each conversation's pool of custom callbacks, by type (daemon/callback_subscriptions.json):
/// session -> type -> what runs it. Hash tables: a callback finds its handler in O(1)
/// (journal#1372).
pub struct Registry {
    path: PathBuf,
    map: HashMap<String, HashMap<String, Registered>>,
}

impl Registry {
    pub fn load(dir: &Path) -> Registry {
        let path = dir.join("callback_subscriptions.json");
        let map = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        Registry { path, map }
    }

    fn save(&self) {
        if let Ok(text) = serde_json::to_string_pretty(&self.map) {
            let _ = super::write_private(&self.path, &text);
        }
    }

    /// Registers the callback's types for the conversation, or takes them off.
    pub fn set(&mut self, session: &str, root: &str, found: &Found, on: bool) {
        let pool = self.map.entry(session.to_string()).or_default();
        pool.retain(|_, r| r.name != found.name || r.root != root);
        if on {
            for kind in &found.types {
                pool.insert(kind.clone(), Registered { custom: true, name: found.name.clone(), root: root.to_string(), handler: found.handler.clone(), args: found.args.clone(), timeout_s: found.timeout_s, folder: found.folder.clone() });
            }
        }
        if pool.is_empty() {
            self.map.remove(session);
        }
        self.save();
    }

    /// What runs `kind` for this conversation, when it is a custom callback registered for it.
    pub fn lookup(&self, session: &str, kind: &str) -> Option<&Registered> {
        self.map.get(session)?.get(kind)
    }

    /// Whether the callback is registered for the conversation, and as the folder says it now.
    pub fn standing(&self, session: &str, root: &str, found: &Found) -> Option<bool> {
        let pool = self.map.get(session)?;
        let mine: Vec<(&String, &Registered)> = pool.iter().filter(|(_, r)| r.name == found.name && r.root == root).collect();
        if mine.is_empty() {
            return None;
        }
        let same = mine.len() == found.types.len() && mine.iter().all(|(kind, r)| found.types.contains(kind) && r.handler == found.handler && r.args == found.args && r.timeout_s == found.timeout_s);
        Some(same)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mt-pcb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[cfg(unix)]
    fn handler(folder: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(folder).unwrap();
        std::fs::write(folder.join("handler"), body).unwrap();
        std::fs::set_permissions(folder.join("handler"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// CON-183: the folders read, with their defaults and their problems; a handler approved,
    /// then changed; a pool per conversation.
    #[cfg(unix)]
    #[test]
    fn project_callbacks_are_read_approved_and_subscribed() {
        let root = project("read");
        let cbs = root.join(".harness/callbacks");
        handler(&cbs.join("echo"), "\x7fELF echo");
        std::fs::write(cbs.join("echo/config.toml"), "handler = \"handler\"\ndescription = \"says it back\"\nargs = [\"--loud\"]\n").unwrap();
        handler(&cbs.join("deploy"), "\x7fELF deploy");
        handler(&cbs.join("script"), "#!/bin/sh\necho hi\n");
        std::fs::write(cbs.join("script/config.toml"), "handler = \"handler\"\n").unwrap();
        std::fs::write(cbs.join("deploy/config.toml"), "handler = \"handler\"\ntypes = [\"deploy_done\", \"deploy_failed\"]\ntimeout_s = 99999\n").unwrap();
        std::fs::create_dir_all(cbs.join("broken")).unwrap();
        std::fs::write(cbs.join("broken/config.toml"), "types = [").unwrap();
        let root_s = root.to_string_lossy().into_owned();
        let all = found(&root_s);
        assert_eq!(all.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["broken", "deploy", "echo", "script"]);
        assert_eq!(all[3].problem.as_deref(), Some(SCRIPT_REFUSED), "a script is refused (journal#1373)");
        let echo = &all[2];
        assert_eq!((echo.types.clone(), echo.args.clone(), echo.timeout_s, echo.problem.clone()), (vec!["echo".to_string()], vec!["--loud".to_string()], 30, None));
        assert_eq!(all[1].timeout_s, MAX_TIMEOUT_S, "a timeout is capped");
        assert!(all[0].problem.as_deref().unwrap().contains("config.toml is not valid"));
        assert_eq!(serving(&root_s, "deploy_failed").map(|f| f.name), Some("deploy".to_string()));
        assert!(serving(&root_s, "nothing").is_none());

        let data = project("data");
        let mut trust = Trust::load(&data);
        assert_eq!(trust.standing(&root_s, echo), Standing::Unapproved);
        assert!(trust.approve(&root_s, &all[0]).is_err(), "a broken one cannot be approved");
        trust.approve(&root_s, echo).unwrap();
        assert_eq!(Trust::load(&data).standing(&root_s, echo), Standing::Approved, "kept on disk");
        std::fs::write(&echo.handler, "\x7fELF changed").unwrap();
        assert_eq!(trust.standing(&root_s, echo), Standing::Changed, "a changed file is not trusted");

        let deploy = &all[1];
        let mut reg = Registry::load(&data);
        assert!(reg.lookup("s1", "deploy_done").is_none(), "nothing is custom before it is registered");
        reg.set("s1", &root_s, deploy, true);
        let r = reg.lookup("s1", "deploy_failed").unwrap();
        assert!(r.custom && r.name == "deploy" && r.handler == deploy.handler, "{r:?}");
        assert!(reg.lookup("s2", "deploy_done").is_none(), "a pool is the conversation's own");
        assert!(Registry::load(&data).lookup("s1", "deploy_done").is_some(), "kept on disk");
        assert_eq!(reg.standing("s1", &root_s, deploy), Some(true));
        let mut moved = deploy.clone();
        moved.types.push("deploy_started".into());
        assert_eq!(reg.standing("s1", &root_s, &moved), Some(false), "the folder changed: to be registered again");
        assert_eq!(reg.standing("s2", &root_s, deploy), None);
        reg.set("s1", &root_s, deploy, false);
        assert!(reg.lookup("s1", "deploy_done").is_none() && reg.lookup("s1", "deploy_failed").is_none());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&data);
    }
}
