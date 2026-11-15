//! A callback URL of each session's own (CON-164): a token per conversation, the same for its whole
//! life, and what came for it while it was not open, kept in order until it opens.
//!
//! Kept in `<data dir>/daemon/session_callbacks.json`, readable only by the user: a token is as
//! secret as a window id.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The most callbacks kept for a session that is not open; beyond it the oldest goes.
pub const MAX_QUEUED: usize = 500;
/// How long a kept callback waits for its session to open.
pub const KEEP_MS: u64 = 7 * 24 * 3600 * 1000;

/// A callback that came while its session was not open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queued {
    pub at_ms: u64,
    pub uuid: String,
    pub label: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub token: String,
    /// The session's project: its conditions and handlers are that project's.
    pub root: String,
    #[serde(default)]
    pub queue: VecDeque<Queued>,
}

#[derive(Debug, Default)]
pub struct Store {
    path: PathBuf,
    /// Conversation id -> its entry.
    pub entries: HashMap<String, Entry>,
}

impl Store {
    pub fn load(dir: &Path) -> Store {
        let path = dir.join("session_callbacks.json");
        let entries = std::fs::read_to_string(&path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
        Store { path, entries }
    }

    pub fn save(&self) {
        if let Ok(text) = serde_json::to_string(&self.entries) {
            let _ = super::write_private(&self.path, &text);
        }
    }

    /// The session's token, made the first time; its project noted as it is now. True when
    /// something changed and the store is to be saved.
    pub fn token_for(&mut self, session: &str, root: &str) -> (String, bool) {
        match self.entries.get_mut(session) {
            Some(entry) if entry.root == root => (entry.token.clone(), false),
            Some(entry) => {
                entry.root = root.to_string();
                (entry.token.clone(), true)
            }
            None => {
                let token = uuid::Uuid::new_v4().to_string();
                self.entries.insert(session.to_string(), Entry { token: token.clone(), root: root.to_string(), queue: VecDeque::new() });
                (token, true)
            }
        }
    }

    /// Keeps a callback for a session not open: the oldest goes past the limit, and is answered.
    pub fn push(&mut self, session: &str, queued: Queued) -> Option<Queued> {
        let entry = self.entries.get_mut(session)?;
        entry.queue.push_back(queued);
        if entry.queue.len() > MAX_QUEUED { entry.queue.pop_front() } else { None }
    }

    /// Drops what waited longer than [`KEEP_MS`]; answers what went.
    pub fn expire(&mut self, now_ms: u64) -> Vec<(String, Queued)> {
        let mut gone = Vec::new();
        for (session, entry) in &mut self.entries {
            while entry.queue.front().is_some_and(|q| now_ms.saturating_sub(q.at_ms) > KEEP_MS) {
                gone.extend(entry.queue.pop_front().map(|q| (session.clone(), q)));
            }
        }
        gone
    }

    /// The sessions with something waiting.
    pub fn waiting(&self) -> Vec<String> {
        self.entries.iter().filter(|(_, e)| !e.queue.is_empty()).map(|(s, _)| s.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mt-session-cb-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn queued(n: u64) -> Queued {
        Queued { at_ms: n, uuid: format!("u{n}"), label: "l".into(), text: format!("t{n}") }
    }

    /// The token is made once and stays, across a save and a load.
    #[test]
    fn a_session_keeps_its_token() {
        let dir = temp("token");
        let mut store = Store::load(&dir);
        let (token, changed) = store.token_for("s1", "/p");
        assert!(changed);
        assert_eq!(store.token_for("s1", "/p"), (token.clone(), false));
        assert_ne!(store.token_for("s2", "/p").0, token, "each session its own");
        store.save();
        let mut again = Store::load(&dir);
        assert_eq!(again.token_for("s1", "/p"), (token, false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Past the limit the oldest goes; past its time a kept callback goes.
    #[test]
    fn the_queue_keeps_the_newest_for_a_week() {
        let dir = temp("queue");
        let mut store = Store::load(&dir);
        store.token_for("s1", "/p");
        for n in 0..MAX_QUEUED as u64 {
            assert_eq!(store.push("s1", queued(n)), None);
        }
        assert_eq!(store.push("s1", queued(MAX_QUEUED as u64)), Some(queued(0)), "the 501st drops the oldest");
        assert_eq!(store.entries["s1"].queue.front(), Some(&queued(1)));
        let gone = store.expire(KEEP_MS + 2);
        assert_eq!(gone.len(), 1, "only the one older than a week: {gone:?}");
        assert_eq!(store.waiting(), ["s1"]);
        assert_eq!(store.push("nobody", queued(1)), None, "a session with no token keeps nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
