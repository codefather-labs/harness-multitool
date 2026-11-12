//! journal#1039: the dashboard's feed is a WebSocket the daemon pushes on, at once on a change,
//! with no polling; a wrong token gets no socket.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use harness_daemon::games::{self, GameSettings, Player};
use harness_daemon::*;
use serde_json::{Value, json};

fn handshake(port: u16, token: &str) -> (TcpStream, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "GET /dashboard/ws?token={token} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
    let mut head = Vec::new();
    let mut b = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        if s.read(&mut b).unwrap() == 0 {
            break;
        }
        head.push(b[0]);
    }
    (s, String::from_utf8_lossy(&head).into_owned())
}

/// The next text message, skipping pings.
fn next_text(s: &mut TcpStream) -> Value {
    loop {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).unwrap();
        let mut len = u64::from(h[1] & 0x7f);
        if len == 126 {
            let mut b = [0u8; 2];
            s.read_exact(&mut b).unwrap();
            len = u64::from(u16::from_be_bytes(b));
        } else if len == 127 {
            let mut b = [0u8; 8];
            s.read_exact(&mut b).unwrap();
            len = u64::from_be_bytes(b);
        }
        assert_eq!(h[1] & 0x80, 0, "a server's frames are not masked");
        let mut payload = vec![0u8; len as usize];
        s.read_exact(&mut payload).unwrap();
        if h[0] & 0x0f == 0x1 {
            return serde_json::from_slice(&payload).unwrap();
        }
    }
}

#[test]
fn the_dashboard_feed_pushes_tables_as_they_change() {
    let data = tempfile::tempdir().unwrap();
    games::write_settings(data.path(), &GameSettings { enabled: true, ..Default::default() }).unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    let (_, refused) = handshake(daemon.port(), "wrong");
    assert!(refused.starts_with("HTTP/1.1 401"), "{refused}");

    let (mut s, head) = handshake(daemon.port(), daemon.admin_token());
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.to_lowercase().contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="), "{head}");
    assert!(head.to_lowercase().contains("upgrade: websocket"), "{head}");
    assert_eq!(next_text(&mut s)["type"], "hello");
    assert_eq!(next_text(&mut s)["type"], "agents");

    // A table made now reaches the page at once, not at the next poll.
    let deliver = |_: &str, _: &str| true;
    let players = vec![Player { session: "s-a".into(), nick: "a".into() }, Player { session: "s-b".into(), nick: "b".into() }];
    let made = Instant::now();
    let id = daemon.games().create("texas_holdem", players, &deliver).unwrap();
    let msg = next_text(&mut s);
    assert!(made.elapsed() < Duration::from_millis(1500), "pushed in {:?}", made.elapsed());
    assert_eq!((msg["type"].as_str(), msg["table"]["id"].as_str(), msg["table"]["state"].as_str()), (Some("table"), Some(id.as_str()), Some("inviting")), "{msg}");

    // Both join: the table plays, and its feed carries the hand's public events.
    daemon.games().op("s-a", "game_join", &json!({"table": id}), &deliver).unwrap();
    let mut msg = next_text(&mut s);
    daemon.games().op("s-b", "game_join", &json!({"table": id}), &deliver).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while msg["table"]["state"] != "playing" && Instant::now() < end {
        msg = next_text(&mut s);
    }
    assert_eq!(msg["table"]["state"], "playing", "{msg}");
    assert_eq!(msg["feed"]["events"][0]["kind"], "hand", "{msg}");
    assert!(!msg.to_string().contains("\"cards\""), "no one's cards: {msg}");
}

/// For watching the dashboard (CON-152): with MT_DASHBOARD_LIVE=<seconds> and MT_DASHBOARD_URL=<file>,
/// a daemon of its own plays a table of four, one move every 1.2 s, and writes the dashboard's
/// address to the file. Skipped otherwise.
#[test]
fn dashboard_live_demo() {
    let (Some(secs), Some(out)) = (std::env::var("MT_DASHBOARD_LIVE").ok().and_then(|s| s.parse::<u64>().ok()), std::env::var_os("MT_DASHBOARD_URL")) else { return };
    let data = tempfile::tempdir().unwrap();
    games::write_settings(data.path(), &GameSettings { enabled: true, ..Default::default() }).unwrap();
    let daemon = Daemon::start(data.path(), 0).unwrap();
    std::fs::write(&out, format!("http://127.0.0.1:{}/dashboard?token={}#T1", daemon.port(), daemon.admin_token())).unwrap();
    let deliver = |_: &str, _: &str| true;
    let names = ["north", "east", "south", "west"];
    let players: Vec<Player> = names.iter().map(|n| Player { session: format!("s-{n}"), nick: n.to_string() }).collect();
    let id = daemon.games().create("texas_holdem", players.clone(), &deliver).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    for p in &players {
        daemon.games().op(&p.session, "game_join", &json!({"table": id}), &deliver).unwrap();
        std::thread::sleep(Duration::from_millis(400));
    }
    let end = Instant::now() + Duration::from_secs(secs);
    let mut n = 0u64;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(1200));
        let t = daemon.games().table(&id).unwrap();
        let Some(to) = t["view"]["to_act"].as_str() else { continue };
        n += 1;
        let state = daemon.games().op(&format!("s-{to}"), "game_state", &json!({"table": id}), &deliver).unwrap();
        let legal: Vec<&str> = state["you"]["legal"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
        let min = state["you"]["min_raise_to"].as_u64().unwrap_or(40);
        let what = if n % 7 == 3 && legal.contains(&"raise") { json!({"do": "raise", "amount": min}) }
            else if n % 9 == 5 && legal.contains(&"bet") { json!({"do": "bet", "amount": min.max(40)}) }
            else if n % 11 == 7 { json!({"do": "fold"}) }
            else if legal.contains(&"check") { json!({"do": "check"}) } else { json!({"do": "call"}) };
        let mut args = what;
        args["table"] = json!(id);
        let _ = daemon.games().op(&format!("s-{to}"), "game_act", &args, &deliver);
    }
}
