//! CON-20: the store behind the daemon answers as it does in-process.

use std::path::Path;

use harness_api::*;
use harness_daemon::client::Client;
use harness_daemon::memory::DaemonMemory;
use harness_daemon::*;
use harness_memory_journal::plugin::JournalMemory;
use harness_memory_journal::project::key_for;

fn now<T>(mut future: BoxFuture<'_, T>) -> T {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("a memory call should be ready"),
    }
}

fn entry(body: &str, flags: &[&str]) -> JournalEntry {
    JournalEntry { blocked_by: None, proofs: Vec::new(), body: body.into(), category: "decision".into(), tags: vec!["t".into()], flags: flags.iter().map(|f| f.to_string()).collect(), salience: None, feature: None }
}

/// The same calls, whatever is behind the interface: what comes back, without its timestamps.
fn exercise(m: &dyn Memory) -> Vec<String> {
    let mut seen = Vec::new();
    let strip = |r: &JournalRecord| (r.id, r.body.clone(), r.category.clone(), r.tags.clone(), r.flags.clone());
    let first = now(m.journal_add(entry("Prices are in cents.", &["specification"]))).unwrap();
    seen.push(format!("{:?}", strip(&first)));
    let todo = now(m.journal_add(entry("an open question", &["question"]))).unwrap();
    now(m.journal_add(entry("an old rule", &["constraint", "deprecated"]))).unwrap();
    now(m.journal_close(todo.id, "done")).unwrap();
    seen.push(format!("{:?}", now(m.journal_get(todo.id)).unwrap().map(|r| strip(&r))));
    seen.push(format!("{:?}", now(m.journal_get(999)).unwrap().map(|r| strip(&r))));
    seen.push(format!("{:?}", now(m.journal_search("cents")).unwrap().iter().map(strip).collect::<Vec<_>>()));
    seen.push(format!("{:?}", now(m.journal_binding(10)).unwrap().iter().map(strip).collect::<Vec<_>>()));
    let map = now(m.map_new(MapKind::Flowchart, "deploy", None)).unwrap();
    let node = NewMapNode { text: "build".into(), kind: "process".into(), ..Default::default() };
    let added = now(m.map_add(map.id, node)).unwrap();
    seen.push(format!("{:?} {:?}", added.nodes.iter().map(|n| n.text.clone()).collect::<Vec<_>>(), added.kind));
    seen.push(format!("{:?}", now(m.map_list()).unwrap()));
    seen.push(format!("{:?}", now(m.map_read(map.id)).unwrap().map(|d| d.title)));
    seen.push(format!("{:?}", now(m.map_read(999)).unwrap().map(|d| d.title)));
    seen.push(format!("{:?}", now(m.journal_close(999, "no")).is_err()));
    seen
}

#[test]
fn every_call_answers_as_it_does_in_process() {
    let (data, root, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let _daemon = Daemon::start(data.path(), 0).unwrap();
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let remote = DaemonMemory::new(client.clone(), data.path().to_path_buf(), &root_path, "s");
    let through = exercise(&remote);

    let direct = JournalMemory::open(local.path(), Path::new("direct"), "s").unwrap();
    assert_eq!(through, exercise(&direct));
    assert!(client.stats().unwrap()["memory_ops"].as_u64().unwrap() >= 14);

    // It really is in the project's own journal.
    let key = key_for(None, Path::new(&root_path));
    let store = JournalMemory::open(&data.path().join("journals"), &key, "s").unwrap();
    assert_eq!(now(store.journal_search("cents")).unwrap().len(), 1);
}

/// A daemon replaced under a session (CON-131): a call made while none answers waits, and goes
/// once the next one is up, applied once.
#[test]
fn a_call_made_while_the_daemon_is_away_waits_and_goes_once() {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let first = Daemon::start(data.path(), 0).unwrap();
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let memory = DaemonMemory::new(client.clone(), data.path().to_path_buf(), &root_path, "s");
    assert_eq!(now(memory.journal_add(entry("before the swap", &[]))).unwrap().id, 1);
    client.shutdown().unwrap();
    drop(first);
    let path = data.path().to_path_buf();
    let starter = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(3));
        Daemon::start(&path, 0).unwrap()
    });
    let started = std::time::Instant::now();
    let record = now(memory.journal_add(entry("across the swap", &[]))).unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_secs(3), "it waited for the next daemon");
    assert!(started.elapsed() < std::time::Duration::from_secs(8), "and went within a try of it: {:?}", started.elapsed());
    assert_eq!(record.id, 2);
    assert_eq!(now(memory.journal_search("swap")).unwrap().len(), 2, "each applied once");
    let _daemon = starter.join().unwrap();
}

/// journal#456 (journal#1132): what a session writes is noted as its own, and when what blocks it
/// closes, that session is told with its next message; the one closing it is not told again.
#[test]
fn a_released_entry_is_told_to_the_session_that_wrote_it() {
    use std::sync::{Arc, Mutex};
    struct Host(Mutex<Vec<(String, String)>>);
    impl ConversationHost for Host {
        fn op(&self, _window: &str, _op: &serde_json::Value) -> Result<serde_json::Value, String> {
            Ok(serde_json::Value::Null)
        }
        fn events(&self, _window: &str, _after: u64, _wait: std::time::Duration) -> Vec<serde_json::Value> {
            Vec::new()
        }
        fn stats(&self) -> serde_json::Value {
            serde_json::Value::Null
        }
        fn close(&self, _only_if_idle: bool) -> bool {
            true
        }
        fn session_of(&self, window: &str, tab: u64) -> Option<String> {
            Some(format!("{window}-{tab}"))
        }
        fn remind(&self, session: &str, _hook: &str, text: &str) -> bool {
            self.0.lock().unwrap().push((session.to_string(), text.to_string()));
            true
        }
    }
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let host = Arc::new(Host(Mutex::new(Vec::new())));
    daemon.host_conversations(host.clone());
    let client = Client::find(data.path()).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let a = DaemonMemory::new(client.clone(), data.path().to_path_buf(), &root_path, "multitool").for_tab("w", 1);
    let b = DaemonMemory::new(client.clone(), data.path().to_path_buf(), &root_path, "multitool").for_tab("w", 2);
    let blocker = now(b.journal_add(entry("which schema?", &["question"]))).unwrap();
    let mut waiting = entry("the migration", &["todo"]);
    waiting.blocked_by = Some(blocker.id);
    let waiting = now(a.journal_add(waiting)).unwrap();
    // B's own entry, blocked by A's: closing it must not tell B about itself.
    let mut own = entry("B's follow-up", &["todo"]);
    own.blocked_by = Some(blocker.id);
    now(b.journal_add(own)).unwrap();
    now(b.journal_close(blocker.id, "the second one")).unwrap();
    let told = host.0.lock().unwrap().clone();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].0, "w-1");
    assert!(told[0].1.contains(&format!("journal#{} is free now", waiting.id)), "{told:?}");
}
