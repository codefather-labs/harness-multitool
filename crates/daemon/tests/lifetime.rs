//! CON-155 (journal#1096, journal#1095): a daemon serves its data directory only while it is the
//! daemon that directory publishes. One whose folder was removed, or whose address another daemon
//! took, stops by itself instead of living on with nothing to serve.

use std::sync::Arc;
use std::time::Duration;

use harness_daemon::Daemon;

/// Whether `daemon` stops within `limit`.
fn stops_within(daemon: &Arc<Daemon>, limit: Duration) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let waiting = daemon.clone();
    std::thread::spawn(move || {
        waiting.wait_for_shutdown();
        let _ = tx.send(());
    });
    rx.recv_timeout(limit).is_ok()
}

#[test]
fn a_daemon_whose_folder_is_removed_stops() {
    let data = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    std::fs::remove_dir_all(data.path()).unwrap();
    assert!(stops_within(&daemon, Duration::from_secs(15)), "it lived on with its folder gone");
    assert!(!data.path().join("daemon").join("address.json").exists(), "and did not bring its address back");
}

#[test]
fn a_daemon_whose_address_another_took_stops() {
    let data = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let path = harness_daemon::address_path(data.path());
    let mut address: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    address["admin"] = "another daemon's token".into();
    std::fs::write(&path, address.to_string()).unwrap();
    assert!(stops_within(&daemon, Duration::from_secs(15)), "it served a folder that names another daemon");
}

#[test]
fn a_daemon_that_is_the_published_one_keeps_serving() {
    let data = tempfile::tempdir().unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    assert!(!stops_within(&daemon, Duration::from_secs(12)), "it stopped with its address intact");
    assert!(!daemon.stopping());
}
