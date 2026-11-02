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
        blocked_by: e.blocked_by.iter().map(|id| *id as u64).collect(),
        blocker_dropped: e.blocker_dropped.iter().map(|id| *id as u64).collect(),
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
                blocked_by: entry.blocked_by.iter().map(|id| *id as i64).collect(),
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

    fn journal_list(&self, filter: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        let conn = self.conn.borrow();
        let wanted = journal::Filter { category: filter.category, flags: filter.flags, open: filter.open, ..Default::default() };
        let listed = journal::list(&conn, &wanted, 0, filter.limit.min(journal::MAX_PAGE_SIZE));
        ready(listed.map(|(_, entries)| entries.into_iter().map(record).collect()).map_err(fail))
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

    fn more(&self, op: MemoryOp) -> BoxFuture<'_, PluginResult<serde_json::Value>> {
        ready(self.do_more(op).map_err(fail))
    }
}

impl JournalMemory {
    fn do_more(&self, op: MemoryOp) -> anyhow::Result<serde_json::Value> {
        use serde_json::json;
        let mut conn = self.conn.borrow_mut();
        let by = Some(self.owner.nick.as_str());
        Ok(match op {
            MemoryOp::JournalList { category, tags, flags, open, feature, offset, limit } => {
                let filter = journal::Filter { category, tags, flags, open, feature, ..Default::default() };
                let (total, entries) = journal::list(&conn, &filter, offset, limit.min(journal::MAX_PAGE_SIZE))?;
                json!({"total": total, "entries": entries})
            }
            MemoryOp::JournalAgenda { limit } => {
                let groups = journal::agenda(&conn, limit.min(journal::MAX_PAGE_SIZE))?;
                json!({"agenda": groups.into_iter().map(|(flag, total, entries)| json!({"flag": flag, "total": total, "entries": entries})).collect::<Vec<_>>()})
            }
            MemoryOp::JournalMark { id, flags } => json!({"id": id, "marked": journal::mark(&mut conn, id as i64, &flags, now())?}),
            MemoryOp::JournalBlock { id, by } => {
                journal::block(&mut conn, id as i64, by as i64)?;
                json!({"id": id, "blocked_by": journal::get(&conn, id as i64)?.map_or(Vec::new(), |e| e.blocked_by)})
            }
            MemoryOp::JournalUnblock { id, by } => {
                let lifted = journal::unblock(&conn, id as i64, by as i64)?;
                json!({"id": id, "lifted": lifted, "blocked_by": journal::get(&conn, id as i64)?.map_or(Vec::new(), |e| e.blocked_by)})
            }
            MemoryOp::JournalUnmark { id, flags } => json!({"id": id, "unmarked": journal::unmark(&mut conn, id as i64, &flags)?}),
            MemoryOp::JournalReopen { id, flags } => json!({"id": id, "reopened": journal::reopen(&mut conn, id as i64, &flags)?}),
            MemoryOp::JournalAbandon { id, resolution } => {
                let abandoned = journal::close_with(&mut conn, id as i64, &[], by, Some(&resolution), now(), journal::CloseMode::Abandon)?;
                let released = journal::released_by(&conn, id as i64)?;
                if released.is_empty() {
                    json!({"id": id, "abandoned": abandoned})
                } else {
                    json!({"id": id, "abandoned": abandoned, "released": released, "note": "These waited for what was just dropped, so what they needed is still not there: re-read each, then lift the link with journal_mark remove, or block it on something else."})
                }
            }
            MemoryOp::JournalReleased { id } => json!({"released": journal::released_by(&conn, id as i64)?}),
            MemoryOp::JournalBranch { id } => match journal::branch(&conn, id as i64)? {
                Some(branch) => serde_json::to_value(branch)?,
                None => anyhow::bail!("entry #{id} belongs to no branch"),
            },
            MemoryOp::JournalLink { from, to, kind } => json!({"linked": journal::link(&mut conn, from as i64, to as i64, &kind)?}),
            MemoryOp::JournalTags { category } => json!({"tags": journal::tags(&conn, category.as_deref())?, "categories": journal::categories(&conn)?}),
            MemoryOp::MapEdit { map, node, text, kind } => {
                mindmap::edit_node(&mut conn, map as i64, node as i64, text.as_deref(), kind.as_deref(), now())?;
                json!({"map": map, "node": node})
            }
            MemoryOp::MapNodeState { map, node, state } => {
                mindmap::set_node_state(&mut conn, map as i64, node as i64, &state, now())?;
                json!({"map": map, "node": node, "state": state})
            }
            MemoryOp::MapLink { map, from, to, label, refs } => {
                let refs: Vec<i64> = refs.iter().map(|r| *r as i64).collect();
                json!({"map": map, "edge": mindmap::add_edge(&mut conn, map as i64, from as i64, to as i64, &label, &refs, now())?})
            }
            MemoryOp::MapRef { map, node, entries } => {
                let entries: Vec<i64> = entries.iter().map(|r| *r as i64).collect();
                json!({"map": map, "node": node, "added": mindmap::add_refs(&mut conn, map as i64, mindmap::Target::Node(node as i64), &entries, now())?})
            }
            MemoryOp::MapClose { map } => {
                mindmap::set_state(&conn, map as i64, false, now())?;
                json!({"closed": map})
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_answers_through_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let m = JournalMemory::open(dir.path(), Path::new("git:github.com/a/b"), "agent").unwrap();
                let entry = JournalEntry { blocked_by: None, body: "chose sqlite because readers block".into(), category: "decision".into(), tags: vec!["sqlite".into()], flags: vec!["question".into()], salience: None, feature: None };
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

    #[test]
    fn the_rest_of_the_store_answers_through_the_interface() {
        let dir = tempfile::tempdir().unwrap();
        let m = JournalMemory::open(dir.path(), Path::new("git:github.com/a/b"), "agent").unwrap();
        let more = |op| pollster_free(m.more(op));
        let entry = |body: &str, flags: &[&str]| JournalEntry { blocked_by: None, body: body.into(), category: "todo".into(), tags: vec!["t".into()], flags: flags.iter().map(|f| f.to_string()).collect(), salience: None, feature: None };
        let todo = pollster_free(m.journal_add(entry("do the thing", &["todo"]))).unwrap().id;
        let other = pollster_free(m.journal_add(entry("a remark", &[]))).unwrap().id;

        let listed = more(MemoryOp::JournalList { category: None, tags: vec![], flags: vec!["todo".into()], open: Some(true), feature: None, offset: 0, limit: 10 }).unwrap();
        assert_eq!((listed["total"].as_i64(), listed["entries"][0]["id"].as_u64()), (Some(1), Some(todo)));
        let agenda = more(MemoryOp::JournalAgenda { limit: 5 }).unwrap();
        assert_eq!(agenda["agenda"][0]["flag"], "todo");
        assert_eq!(more(MemoryOp::JournalMark { id: other, flags: vec!["important".into()] }).unwrap()["marked"][0], "important");
        assert_eq!(more(MemoryOp::JournalUnmark { id: other, flags: vec!["important".into()] }).unwrap()["unmarked"][0], "important");
        assert!(more(MemoryOp::JournalMark { id: other, flags: vec!["nonsense".into()] }).is_err(), "the store's refusals reach the agent");
        assert!(more(MemoryOp::JournalBranch { id: todo }).unwrap().to_string().contains("do the thing"));
        assert_eq!(more(MemoryOp::JournalLink { from: other, to: todo, kind: "cites".into() }).unwrap()["linked"], true);
        assert_eq!(more(MemoryOp::JournalTags { category: None }).unwrap()["tags"][0]["name"], "t");
        assert!(pollster_free(m.journal_close(todo, "no evidence")).is_err(), "a todo still closes only with evidence");
        let (first, second) = (pollster_free(m.journal_add(entry("blocker", &["question"]))).unwrap().id, pollster_free(m.journal_add(entry("blocked", &["question"]))).unwrap().id);
        assert_eq!(more(MemoryOp::JournalBlock { id: second, by: first }).unwrap()["blocked_by"][0], first);
        assert_eq!(pollster_free(m.journal_get(second)).unwrap().unwrap().blocked_by, [first]);
        let refused = pollster_free(m.journal_close(second, "answered")).unwrap_err().to_string();
        assert!(refused.contains("blocked by"), "{refused}");
        assert_eq!(more(MemoryOp::JournalUnblock { id: second, by: first }).unwrap()["lifted"], true);
        pollster_free(m.journal_close(second, "answered")).unwrap();
        let third = JournalEntry { blocked_by: Some(first), ..entry("waits at birth", &["question"]) };
        let third = pollster_free(m.journal_add(third)).unwrap();
        assert_eq!(third.blocked_by, [first], "an entry says what holds it as soon as it is written");
        assert_eq!(more(MemoryOp::JournalAbandon { id: todo, resolution: "dropped".into() }).unwrap()["abandoned"][0], "todo");
        assert_eq!(more(MemoryOp::JournalReopen { id: todo, flags: vec![] }).unwrap()["reopened"][0], "todo");

        let map = pollster_free(m.map_new(MapKind::MindMap, "topic", None)).unwrap();
        let added = pollster_free(m.map_add(map.id, NewMapNode { text: "a".into(), ..Default::default() })).unwrap();
        let added = pollster_free(m.map_add(added.id, NewMapNode { text: "b".into(), ..Default::default() })).unwrap();
        let nodes: Vec<u64> = added.nodes.iter().filter(|n| n.text != "topic").map(|n| n.id).collect();
        more(MemoryOp::MapEdit { map: map.id, node: nodes[0], text: Some("a2".into()), kind: Some("fact".into()) }).unwrap();
        more(MemoryOp::MapNodeState { map: map.id, node: nodes[0], state: "done".into() }).unwrap();
        more(MemoryOp::MapLink { map: map.id, from: nodes[0], to: nodes[1], label: "leads to".into(), refs: vec![todo] }).unwrap();
        assert_eq!(more(MemoryOp::MapRef { map: map.id, node: nodes[1], entries: vec![other] }).unwrap()["added"], 1);
        let read = pollster_free(m.map_read(map.id)).unwrap().unwrap();
        let first = read.nodes.iter().find(|n| n.id == nodes[0]).unwrap();
        assert_eq!((first.text.as_str(), first.state.as_str()), ("a2", "done"));
        assert_eq!(read.edges[0].label, "leads to");
        more(MemoryOp::MapClose { map: map.id }).unwrap();
        assert!(!pollster_free(m.map_list()).unwrap().iter().any(|(id, _)| *id == map.id), "a closed map is off the list");
    }
}
