//! A whole journal in and out, as a stream of JSON documents: `{"entries": [...]}` and
//! `{"map": {...}}`. Ids are kept, so what cites an entry or a node still means it.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::{journal, mindmap};

/// What an import did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub new: usize,
    pub updated: usize,
    pub maps: usize,
    /// Where the journal that was here went.
    pub aside: Option<String>,
}

/// Brings this stream into the journal of `key`. With `aside`, the journal there is moved out of
/// the way first, renamed and not deleted, and only if it is not the one being updated.
pub fn import(dir: &Path, key: &Path, text: &str, aside: bool) -> Result<Report> {
    let mut entries: Vec<journal::ImportedEntry> = Vec::new();
    let mut maps: Vec<mindmap::Map> = Vec::new();
    for document in serde_json::Deserializer::from_str(text).into_iter::<Value>() {
        let mut document = document.context("not JSON")?;
        if let Some(list) = document.get_mut("entries").map(Value::take) {
            entries.extend(serde_json::from_value::<Vec<journal::ImportedEntry>>(list).context("an entry")?);
        } else if let Some(map) = document.get_mut("map").map(Value::take) {
            maps.push(serde_json::from_value(map).context("a map")?);
        }
    }
    entries.sort_by_key(|e| e.id);
    entries.dedup_by_key(|e| e.id);
    let mut report = Report::default();
    if aside {
        report.aside = journal::retire(dir, key, "before-import")?.map(|p| p.display().to_string());
    }
    let mut conn = journal::open(dir, key, true)?.context("no journal")?;
    (report.new, report.updated) = journal::import_exact(&mut conn, &entries)?;
    for map in &maps {
        mindmap::import_exact(&mut conn, map, journal::now_secs())?;
    }
    report.maps = maps.len();
    Ok(report)
}

/// The journal of `key` as such a stream.
pub fn export(dir: &Path, key: &Path) -> Result<String> {
    let conn = journal::open(dir, key, false)?.context("no journal here")?;
    let mut out = String::new();
    let mut offset = 0;
    loop {
        let (total, page) = journal::list(&conn, &journal::Filter::default(), offset, 100)?;
        out.push_str(&json!({"entries": page, "total": total}).to_string());
        out.push('\n');
        offset += page.len();
        if page.is_empty() || offset as i64 >= total {
            break;
        }
    }
    for map in mindmap::export_all(&conn)? {
        out.push_str(&json!({"map": map}).to_string());
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(body: &str) -> String {
        let entries = json!({"entries": [
            {"id": 5, "created_at": 100, "category": "todo", "body": "first", "tags": ["a"], "salience": "high", "agent": "x", "feature": "f",
             "flags": [{"flag": "todo", "open": false, "set_at": 100, "closed_at": 101, "closed_by": "x", "resolution": "done"}], "links": [{"dir": "in", "id": 9, "kind": "fixes"}]},
            {"id": 9, "created_at": 102, "category": "update", "body": body, "tags": ["a", "b"], "salience": "medium",
             "flags": [{"flag": "unverified", "open": true, "set_at": 102}], "links": [{"dir": "out", "id": 5, "kind": "fixes"}], "proofs": ["log.txt"]}
        ]});
        let map = json!({"map": {"id": 3, "title": "t", "owner_nick": "x", "owner_host": null, "state": "open", "shared": true, "layout": "mindmap", "direction": "down", "node_count": 2, "open_nodes": 2, "created_at": 100, "updated_at": 102, "root": 38,
            "nodes": [{"id": 38, "parent": null, "kind": "topic", "text": "t", "state": "open", "refs": [], "author": null}, {"id": 39, "parent": 38, "kind": "fact", "text": "n", "state": "done", "refs": [9], "author": "x"}],
            "edges": [], "chains": []}});
        format!("{entries}\n{map}\n")
    }

    #[test]
    fn a_journal_comes_in_under_its_ids_and_goes_out_the_same() {
        let dir = std::env::temp_dir().join(format!("mt-transfer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let key = Path::new("git:example/repo");
        // What was here goes aside.
        let mut conn = journal::open(&dir, key, true).unwrap().unwrap();
        let old = journal::ImportedEntry { id: 5, created_at: 1, category: "note".into(), body: "other".into(), tags: vec![], salience: "low".into(), agent: None, agent_id: None, session_id: None, feature: None, source_artifact: None, flags: vec![], links: vec![], proofs: vec![] };
        journal::import_exact(&mut conn, &[old]).unwrap();
        drop(conn);
        assert!(import(&dir, key, &stream("second"), false).is_err(), "another text under an id is refused");
        let report = import(&dir, key, &stream("second"), true).unwrap();
        assert_eq!((report.new, report.updated, report.maps), (2, 0, 1));
        assert!(report.aside.as_deref().is_some_and(|p| std::path::Path::new(p).exists()), "moved, not deleted");

        let conn = journal::open(&dir, key, false).unwrap().unwrap();
        let first = journal::get(&conn, 5).unwrap().unwrap();
        assert_eq!(first.flags[0].resolution.as_deref(), Some("done"));
        assert!(!first.flags[0].open, "closed stays closed");
        assert!(first.links.iter().any(|l| l.id == 9), "a link shows from both ends");
        let map = mindmap::load(&conn, 3).unwrap().unwrap();
        assert_eq!((map.root, map.nodes[1].id, map.nodes[1].refs.clone(), map.nodes[1].state.as_str()), (38, 39, vec![9], "done"));
        drop(conn);

        // Again, with more: an update, and nothing twice.
        let again = import(&dir, key, &stream("second"), false).unwrap();
        assert_eq!((again.new, again.updated), (0, 2));
        let out = export(&dir, key).unwrap();
        assert!(out.contains("\"first\"") && out.contains("\"map\""));
        // What went out comes in the same somewhere else.
        let other = Path::new("git:example/other");
        let copy = import(&dir, other, &out, false).unwrap();
        assert_eq!((copy.new, copy.maps), (2, 1));
        assert_eq!(export(&dir, other).unwrap().lines().count(), out.lines().count());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
