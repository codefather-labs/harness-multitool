//! The session log as JSON lines, one file per session (IF-9, INV-5): only ever appended.

use std::rc::Rc;

use harness_api::event::{from_line, to_line};
use harness_api::platform::Fs;
use harness_api::*;

pub struct FileSessions {
    fs: Rc<dyn Fs>,
    dir: String,
}

impl FileSessions {
    pub fn new(fs: Rc<dyn Fs>, dir: &str) -> Self {
        FileSessions { fs, dir: dir.trim_end_matches('/').to_string() }
    }

    fn path(&self, session: &str) -> String {
        format!("{}/{session}.jsonl", self.dir)
    }
}

impl SessionStore for FileSessions {
    fn append<'a>(&'a self, session: &'a str, entries: &'a [LogEntry]) -> BoxFuture<'a, PluginResult<()>> {
        Box::pin(async move {
            let mut text = String::new();
            for entry in entries {
                text.push_str(&to_line(entry));
                text.push('\n');
            }
            self.fs.append(&self.path(session), text.as_bytes()).await
        })
    }

    fn load<'a>(&'a self, session: &'a str) -> BoxFuture<'a, PluginResult<Vec<LogEntry>>> {
        Box::pin(async move {
            let bytes = self.fs.read(&self.path(session)).await?;
            let text = String::from_utf8_lossy(&bytes);
            let mut entries = Vec::new();
            let lines: Vec<&str> = text.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                match from_line(line) {
                    Ok(entry) => entries.push(entry),
                    // A crash can cut the last line; anything else is damage to report.
                    Err(_) if i + 1 == lines.len() => break,
                    Err(e) => return Err(PluginError::new(format!("session {session}, line {}: {e}", i + 1))),
                }
            }
            Ok(entries)
        })
    }

    fn list(&self) -> BoxFuture<'_, PluginResult<Vec<String>>> {
        Box::pin(async move {
            let names = self.fs.list(&self.dir).await.unwrap_or_default();
            Ok(names.iter().filter_map(|n| n.strip_suffix(".jsonl").map(str::to_string)).collect())
        })
    }
}
