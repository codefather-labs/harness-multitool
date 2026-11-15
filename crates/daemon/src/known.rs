//! The register of conversations the daemon has hosted (CON-167): where each lives, what it is
//! called, and when it was last open, so a closed one can be opened again from Telegram.
//!
//! Kept in `<data dir>/daemon/known.json`, readable only by the user.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::OpenSession;

/// The most conversations kept; the ones open longest ago go first.
const MAX: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Known {
    pub session: String,
    /// The project folder it is open in.
    pub root: String,
    pub title: String,
    #[serde(default)]
    pub nick: Option<String>,
    pub last_open_ms: u64,
}

impl Known {
    /// The project's name, as a person reads it: its folder's last part.
    pub fn project(&self) -> &str {
        Path::new(&self.root).file_name().and_then(|n| n.to_str()).unwrap_or(&self.root)
    }
}

fn path(dir: &Path) -> std::path::PathBuf {
    dir.join("known.json")
}

/// Every known conversation, the one open most lately first.
pub fn load(dir: &Path) -> Vec<Known> {
    let mut all: Vec<Known> = std::fs::read_to_string(path(dir)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    all.sort_by_key(|k| std::cmp::Reverse(k.last_open_ms));
    all
}

/// Notes the conversations open now: each is added or brought up to date, open as of `now_ms`.
/// Written only when something changed besides the time, or a minute has passed.
pub fn note(dir: &Path, open: &[OpenSession], now_ms: u64) -> bool {
    let mut all = load(dir);
    let mut changed = false;
    for session in open.iter().filter(|s| !s.root.is_empty()) {
        match all.iter_mut().find(|k| k.session == session.id) {
            Some(k) => {
                if k.root != session.root || k.title != session.title || k.nick != session.nick {
                    changed = true;
                }
                if now_ms.saturating_sub(k.last_open_ms) >= 60_000 {
                    changed = true;
                }
                k.root = session.root.clone();
                k.title = session.title.clone();
                k.nick = session.nick.clone();
                k.last_open_ms = now_ms;
            }
            None => {
                all.push(Known { session: session.id.clone(), root: session.root.clone(), title: session.title.clone(), nick: session.nick.clone(), last_open_ms: now_ms });
                changed = true;
            }
        }
    }
    if !changed {
        return false;
    }
    all.sort_by_key(|k| std::cmp::Reverse(k.last_open_ms));
    all.truncate(MAX);
    if let Ok(text) = serde_json::to_string(&all) {
        let _ = crate::write_private(&path(dir), &text);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(id: &str, root: &str, title: &str) -> OpenSession {
        OpenSession { id: id.into(), title: title.into(), nick: None, root: root.into() }
    }

    /// A conversation open is kept, brought up to date, and stays when it closes.
    #[test]
    fn the_register_keeps_what_was_open() {
        let dir = tempfile::tempdir().unwrap();
        assert!(note(dir.path(), &[open("s1", "/p/alpha", "first")], 1_000));
        assert!(!note(dir.path(), &[open("s1", "/p/alpha", "first")], 2_000), "nothing new within a minute");
        let mut named = open("s1", "/p/alpha", "nick-1");
        named.nick = Some("nick-1".into());
        assert!(note(dir.path(), &[named, open("s2", "/p/beta", "second")], 3_000));
        assert!(!note(dir.path(), &[], 4_000), "a close changes nothing");
        let all = load(dir.path());
        assert_eq!(all.iter().map(|k| k.session.as_str()).collect::<Vec<_>>(), ["s1", "s2"]);
        assert_eq!(all[0].nick.as_deref(), Some("nick-1"));
        assert_eq!(all[1].project(), "beta");
        assert!(note(dir.path(), &[open("s2", "/p/beta", "second")], 70_000), "the time, after a minute");
        assert_eq!(load(dir.path())[0].session, "s2", "the one open most lately first");
        assert!(!note(dir.path(), &[open("s3", "", "rootless")], 80_000), "with no root, nothing to open it in");
    }
}
