//! The store behind the `Memory` interface (IF-7), in-process until the daemon slice.

use std::cell::RefCell;
use std::path::Path;

use harness_api::*;
use rusqlite::Connection;

use crate::{journal, mindmap};

pub struct JournalMemory {
    conn: RefCell<Connection>,
    owner: mindmap::Owner,
}

fn fail(e: anyhow::Error) -> PluginError {
    PluginError::new(format!("{e:#}"))
}

fn now() -> i64 {
    journal::now_secs()
}

fn record(e: journal::Entry) -> JournalRecord {
    JournalRecord {
        id: e.id as u64,
        created: e.created.clone(),
        category: e.category.clone(),
        tags: e.tags.clone(),
        body: e.body.clone(),
        flags: e.flags.iter().map(|f| f.flag.clone()).collect(),
    }
}

fn map_data(map: mindmap::Map) -> MapData {
    MapData {
        id: map.summary.id as u64,
        title: map.summary.title.clone(),
        kind: if map.summary.layout == "flowchart" { MapKind::Flowchart } else { MapKind::MindMap },
        direction: map.summary.direction.clone(),
        root: map.root as u64,
        nodes: map
            .nodes
            .iter()
            .map(|n| MapNode { id: n.id as u64, parent: n.parent.map(|p| p as u64), kind: n.kind.clone(), text: n.text.clone(), state: n.state.clone(), refs: n.refs.iter().map(|r| *r as u64).collect() })
            .collect(),
        edges: map.edges.iter().map(|e| MapEdge { from: e.from as u64, to: e.to as u64, label: e.label.clone(), refs: e.refs.iter().map(|r| *r as u64).collect() }).collect(),
    }
}

fn ready<'a, T: 'a>(value: T) -> BoxFuture<'a, T> {
    Box::pin(std::future::ready(value))
}

impl JournalMemory {
    /// Opens the journal of `key` under `dir`, creating it when new.
    pub fn open(dir: &Path, key: &Path, owner: &str) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
        let conn = journal::open(dir, key, true).map_err(|e| format!("{e:#}"))?.ok_or("the journal did not open")?;
        Ok(JournalMemory { conn: RefCell::new(conn), owner: mindmap::Owner { nick: owner.to_string(), host: None } })
    }

    fn read(&self, id: u64) -> PluginResult<MapData> {
        let map = mindmap::load(&self.conn.borrow(), id as i64).map_err(fail)?;
        map.map(map_data).ok_or_else(|| PluginError::new(format!("no map #{id}")))
    }
}

impl Memory for JournalMemory {
    fn journal_add(&self, entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
        let result = (|| {
            let new = journal::NewEntry {
                body: entry.body,
                category: entry.category,
                tags: journal::normalize_tags(&entry.tags),
                salience: journal::normalize_salience(entry.salience.as_deref())?,
                feature: entry.feature,
                flags: journal::normalize_flags(&entry.flags)?,
                ..Default::default()
            };
            let mut conn = self.conn.borrow_mut();
            let (_, id) = journal::add(&mut conn, &new, now())?;
            journal::get(&conn, id)?.map(record).ok_or_else(|| anyhow::anyhow!("the entry was not stored"))
        })();
        ready(result.map_err(fail))
    }

    fn journal_search(&self, query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        let result = journal::search(&self.conn.borrow(), query, None, journal::SearchMode::Lexical, &journal::Filter::default(), 10);
        ready(result.map(|(_, hits)| hits.into_iter().map(|h| record(h.entry)).collect()).map_err(fail))
    }

    fn journal_get(&self, id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
        ready(journal::get(&self.conn.borrow(), id as i64).map(|e| e.map(record)).map_err(fail))
    }

    fn journal_close(&self, id: u64, resolution: &str) -> BoxFuture<'_, PluginResult<()>> {
        let result = journal::close(&mut self.conn.borrow_mut(), id as i64, &[], Some(&self.owner.nick), Some(resolution), now());
        ready(result.map(|_| ()).map_err(fail))
    }

    fn journal_binding(&self, limit: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        let conn = self.conn.borrow();
        let filter = journal::Filter { flags: vec!["specification".into(), "constraint".into()], ..Default::default() };
        let listed = journal::list(&conn, &filter, 0, limit.min(journal::MAX_PAGE_SIZE));
        let result = listed.map(|(_, entries)| entries.into_iter().filter(|e| !e.flags.iter().any(|f| f.flag == "deprecated")).map(record).collect());
        ready(result.map_err(fail))
    }

    fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>> {
        let listed = mindmap::list(&self.conn.borrow(), Some(&self.owner), false);
        ready(listed.map(|maps| maps.into_iter().map(|m| (m.id as u64, m.title)).collect()).map_err(fail))
    }

    fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
        let spec = mindmap::NewMap {
            title,
            layout: match kind {
                MapKind::MindMap => "mindmap",
                MapKind::Flowchart => "flowchart",
            },
            direction: direction.unwrap_or("right"),
            // A map of a project belongs to its agents together.
            shared: true,
        };
        let created = mindmap::create_map(&mut self.conn.borrow_mut(), &spec, &self.owner, now());
        ready(created.map_err(fail).and_then(|(id, _)| self.read(id as u64)))
    }

    fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
        let refs: Vec<i64> = node.refs.iter().map(|r| *r as i64).collect();
        let spec = mindmap::NewNode {
            parent: node.parent.map(|p| p as i64),
            after: node.after.map(|a| a as i64),
            label: &node.label,
            kind: if node.kind.is_empty() { "note" } else { &node.kind },
            text: &node.text,
            refs: &refs,
            author: Some(&self.owner.nick),
        };
        let added = mindmap::add(&mut self.conn.borrow_mut(), map as i64, &spec, now());
        ready(added.map_err(fail).and_then(|_| self.read(map)))
    }

    fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
        let loaded = mindmap::load(&self.conn.borrow(), map as i64);
        ready(loaded.map(|m| m.map(map_data)).map_err(fail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_answers_through_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let m = JournalMemory::open(dir.path(), Path::new("git:github.com/a/b"), "agent").unwrap();
                let entry = JournalEntry { body: "chose sqlite because readers block".into(), category: "decision".into(), tags: vec!["sqlite".into()], flags: vec!["question".into()], salience: None, feature: None };
        let added = pollster_free(m.journal_add(entry)).unwrap();
        assert_eq!(added.id, 1);
        assert_eq!(added.flags, vec!["question"]);
        assert_eq!(pollster_free(m.journal_search("sqlite")).unwrap()[0].id, 1);
        pollster_free(m.journal_close(1, "done")).unwrap();
        assert!(pollster_free(m.journal_get(1)).unwrap().is_some());

        let chart = pollster_free(m.map_new(MapKind::Flowchart, "deploy", Some("down"))).unwrap();
        assert_eq!((chart.kind, chart.direction.as_str()), (MapKind::Flowchart, "down"));
        let next = NewMapNode { text: "build".into(), kind: "process".into(), after: Some(chart.root), ..Default::default() };
        let chart = pollster_free(m.map_add(chart.id, next)).unwrap();
        assert_eq!(chart.nodes.len(), 2);
        assert_eq!(chart.edges.len(), 1);
        assert!(pollster_free(m.map_read(99)).unwrap().is_none());
    }

    /// The plugin's futures are all ready at once; no executor is needed.
    fn pollster_free<T>(mut f: BoxFuture<'_, T>) -> T {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match f.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(v) => v,
            std::task::Poll::Pending => panic!("the plugin's future was not ready"),
        }
    }
}
