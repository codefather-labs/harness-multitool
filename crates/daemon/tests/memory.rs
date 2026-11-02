//! CON-20: the store behind the daemon answers as it does in-process.

use std::path::Path;

use harness_api::*;
use harness_daemon::client::Client;
use harness_daemon::memory::{CallError, Call, DaemonMemory, Request};
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

#[test]
fn a_wrong_token_stores_nothing() {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let root_path = root.path().to_string_lossy().into_owned();
    let stranger = Client::new(&Address { port: daemon.port(), admin: "wrong".into(), pid: 0 });
    let call = Call::JournalAdd { entry: entry("sneaked in", &[]) };
    let answer = stranger.memory(&Request { root: root_path.clone(), owner: "s".into(), call });
    assert!(matches!(answer, Err(CallError::Stale(_))), "{answer:?}");
    let key = key_for(None, Path::new(&root_path));
    let store = JournalMemory::open(&data.path().join("journals"), &key, "s").unwrap();
    assert!(now(store.journal_search("sneaked")).unwrap().is_empty());
    assert_eq!(Client::find(data.path()).unwrap().stats().unwrap()["memory_ops"], 0);
}

/// A daemon replaced under a session: another port and token, found again from the address file.
#[test]
fn a_call_outlives_the_daemon_being_replaced() {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = Client::new(&Address { port: free, admin: "old".into(), pid: 0 });
    let root_path = root.path().to_string_lossy().into_owned();
    let memory = DaemonMemory::new(gone, data.path().to_path_buf(), &root_path, "s");
    let path = data.path().to_path_buf();
    let starter = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        Daemon::start(&path, 0).unwrap()
    });
    let record = now(memory.journal_add(entry("across the swap", &[]))).unwrap();
    assert_eq!(record.id, 1);
    assert_eq!(now(memory.journal_search("swap")).unwrap().len(), 1, "applied once");
    let _daemon = starter.join().unwrap();
}

#[test]
fn no_daemon_is_an_error_not_a_hang() {
    let (data, root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = Client::new(&Address { port: free, admin: "old".into(), pid: 0 });
    let memory = DaemonMemory::new(gone, data.path().to_path_buf(), &root.path().to_string_lossy(), "s");
    let started = std::time::Instant::now();
    let answer = now(memory.journal_get(1));
    assert!(answer.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(15), "{:?}", started.elapsed());
}
