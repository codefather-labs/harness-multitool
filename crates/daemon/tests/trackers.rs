//! ADR 0028, CON-139 to CON-143: journal todos go to the project's tracker through the daemon,
//! against a fake gh that records what it was asked, with which GH_CONFIG_DIR, and answers as gh.

use std::time::{Duration, Instant};

use harness_api::*;
use harness_daemon::client::Client;
use harness_daemon::memory::DaemonMemory;
use harness_daemon::trackers::{self, Settings};
use harness_daemon::*;

fn now<T>(mut future: BoxFuture<'_, T>) -> T {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match future.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(value) => value,
        std::task::Poll::Pending => panic!("a memory call should be ready"),
    }
}

fn entry(body: &str, flags: &[&str]) -> JournalEntry {
    JournalEntry { blocked_by: None, proofs: Vec::new(), body: body.into(), category: "todo".into(), tags: vec!["t".into()], flags: flags.iter().map(|f| f.to_string()).collect(), salience: None, feature: None }
}

#[cfg(unix)]
#[test]
fn a_todo_opens_an_issue_its_branch_comments_and_its_close_closes() {
    use std::os::unix::fs::PermissionsExt;
    let (data, root, bin) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root_path = root.path().to_string_lossy().into_owned();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-C", &root_path]).args(args).output().unwrap().status.success());
    git(&["init", "-q"]);
    git(&["remote", "add", "origin", "git@github.com:acme/widgets.git"]);
    let log = bin.path().join("gh.log");
    let fake = bin.path().join("gh");
    std::fs::write(&fake, format!(r#"#!/bin/sh
input=""
case "$*" in *--body-file*) input=$(cat);; esac
printf '%s|%s|%s\n' "$GH_CONFIG_DIR" "$*" "$input" | tr '\n' ' ' >> {log}
echo >> {log}
case "$1 $2" in
  "api user") echo tester;;
  "repo view") echo "PRIVATE WRITE";;
  "issue create") echo "https://github.com/acme/widgets/issues/7";;
esac
"#, log = log.display())).unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let daemon = Daemon::start(data.path(), 0).unwrap();
    let folder = daemon.tracker_folder(&root_path);
    let gh_dir = bin.path().join("gh-config");
    trackers::write_settings(&folder, &Settings { enabled: true, tracker: None, gh_config_dir: Some(gh_dir.clone()), gh_binary: Some(fake.to_string_lossy().into_owned()), public_ok: false }).unwrap();
    let client = Client::find(data.path()).unwrap();
    let memory = DaemonMemory::new(client, data.path().to_path_buf(), &root_path, "s");
    let calls = || std::fs::read_to_string(&log).unwrap_or_default();
    let until = |what: &str, check: &dyn Fn(&str) -> bool| {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end && !check(&calls()) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(check(&calls()), "{what}:\n{}", calls());
    };

    let todo = now(memory.journal_add(entry("Ship the widget\nmore words", &["todo"]))).unwrap();
    until("the todo opened an issue", &|c| c.contains("issue create --repo acme/widgets --title journal#1: Ship the widget"));
    // gh logs the call before it answers; the map is written after the answer: wait for it.
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && !trackers::issues(&folder).contains_key(&todo.id) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(trackers::issues(&folder).get(&todo.id), Some(&7));
    let built = now(memory.journal_add(entry(&format!("Implements journal#{}: the widget is built", todo.id), &["unverified"]))).unwrap();
    until("the implementation commented on it", &|c| c.contains(&format!("issue comment 7 --repo acme/widgets --body-file -|**journal#{}**", built.id)));
    // The verification names the implementation only: its todo is found through its branch.
    let mut verifies = entry(&format!("Verifies journal#{}: ran the widget's test three times on this machine, all passed with the expected output", built.id), &["verified"]);
    verifies.proofs = vec!["cargo test widget: 3 passed".into()];
    let verified = now(memory.journal_add(verifies)).unwrap();
    until("the verification commented on it, through its branch", &|c| c.contains(&format!("issue comment 7 --repo acme/widgets --body-file -|**journal#{}**", verified.id)));
    now(memory.journal_close(todo.id, "verified: the widget ships")).unwrap();
    until("the close closed it", &|c| c.contains("issue close 7 --repo acme/widgets --reason completed --comment Closed in the journal: verified: the widget ships"));
    // Every call named its gh folder (CON-141).
    assert!(calls().lines().all(|l| l.starts_with(&gh_dir.to_string_lossy().into_owned())), "{}", calls());

    // Off: nothing is queued, nothing is sent.
    trackers::write_settings(&folder, &Settings { enabled: false, ..trackers::read_settings(&folder) }).unwrap();
    let before = calls().lines().count();
    now(memory.journal_add(entry("Another todo", &["todo"]))).unwrap();
    std::thread::sleep(trackers::TICK * 2);
    assert_eq!(calls().lines().count(), before, "off sends nothing: {}", calls());
    assert!(std::fs::read_to_string(folder.join("queue.jsonl")).unwrap_or_default().trim().is_empty(), "nothing queued");
}

/// The trackers not built yet say so (CON-143).
#[test]
fn the_other_trackers_say_they_are_not_built_yet() {
    let folder = tempfile::tempdir().unwrap();
    for name in ["gitlab", "asana", "clickup"] {
        let t = trackers::tracker(folder.path(), &Settings { tracker: Some(name.into()), ..Default::default() });
        assert_eq!(t.name(), name);
        assert!(t.connected().unwrap_err().contains("not implemented yet"));
        assert!(t.open("x/y", "t", "b").is_err());
    }
}
