//! One SQLite database per hand of a game (CON-149, journal#1008): everything that happened in it,
//! and the source of truth for the table's state. A table's state is its game replayed from the
//! seed, the players and the rules of hand-1, with the actions of hand-1, hand-2, ... in order.
//!
//! `<data dir>/games/tables/<table>/hand-<n>.sqlite`:
//! - `meta(key, value)`: table, game, hand, seed, players, rules, began_ms;
//! - `actions(seq, player, action, at_ms)`: every action taken in the hand, as given; a timeout the
//!   rules took for a player who left is `{"do": "timeout"}`;
//! - `events(seq, kind, to_player, text, data, at_ms)`: every event the rules gave, private ones
//!   (a player's cards) with their player, public ones with none.
//!
//! One step (an action and what it set off) is written in one transaction, across the hands it
//! touched: the files are attached to one connection, which SQLite commits atomically in its
//! rollback-journal mode, the default kept here.

use std::path::{Path, PathBuf};

use harness_games::Event;
use harness_games::holdem::{Holdem, Rules};
use harness_games::Game;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// What a table's hands are replayed from: written into every hand's meta, read from hand-1's.
#[derive(Debug, Clone)]
pub struct Meta {
    pub table: String,
    pub game: String,
    pub seed: u64,
    pub players: Vec<String>,
    pub rules: Rules,
}

/// The action of a timeout the rules took.
pub fn timeout() -> Value {
    json!({"do": "timeout"})
}

pub fn path(dir: &Path, hand: u32) -> PathBuf {
    dir.join(format!("hand-{hand}.sqlite"))
}

/// The highest hand with a database, 0 when there is none.
pub fn last_hand(dir: &Path) -> u32 {
    let mut n = 0;
    while path(dir, n + 1).exists() {
        n += 1;
    }
    n
}

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS {s}meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS {s}actions (seq INTEGER PRIMARY KEY, player TEXT NOT NULL, action TEXT NOT NULL, at_ms INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS {s}events (seq INTEGER PRIMARY KEY, kind TEXT NOT NULL, to_player TEXT, text TEXT NOT NULL, data TEXT NOT NULL, at_ms INTEGER NOT NULL);";

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// Writes one step in one transaction: `action` (a player's, taken in hand `hand`) and `events`,
/// each into the hand it belongs to. An event goes to the hand the rules last opened with a
/// `hand` event; those before the first hand (the table's start) go to hand 1. An error means
/// nothing was written, and the step must not be taken.
pub fn record(dir: &Path, meta: &Meta, hand: u32, action: Option<(&str, &Value)>, events: &[Event]) -> Result<(), String> {
    let hand = hand.max(1);
    let mut placed: Vec<(u32, &Event)> = Vec::new();
    let mut current = hand;
    for event in events {
        if event.kind == "hand"
            && let Some(n) = event.data["hand"].as_u64()
        {
            current = n as u32;
        }
        placed.push((current, event));
    }
    let mut hands: Vec<u32> = std::iter::once(hand).chain(placed.iter().map(|(n, _)| *n)).collect();
    hands.sort_unstable();
    hands.dedup();
    let failed = |e: &dyn std::fmt::Display| format!("the hand's database in {} could not be written: {e}", dir.display());
    std::fs::create_dir_all(dir).map_err(|e| failed(&e))?;
    let mut conn = Connection::open(path(dir, hands[0])).map_err(|e| failed(&e))?;
    let schema_of = |n: u32| if n == hands[0] { "main".to_string() } else { format!("h{n}") };
    for &n in &hands[1..] {
        conn.execute("ATTACH DATABASE ?1 AS ?2", params![path(dir, n).to_string_lossy(), schema_of(n)]).map_err(|e| failed(&e))?;
    }
    let tx = conn.transaction().map_err(|e| failed(&e))?;
    let at = now_ms();
    for &n in &hands {
        let s = format!("{}.", schema_of(n));
        tx.execute_batch(&SCHEMA.replace("{s}", &s)).map_err(|e| failed(&e))?;
        let rows = [
            ("table", meta.table.clone()),
            ("game", meta.game.clone()),
            ("hand", n.to_string()),
            ("seed", meta.seed.to_string()),
            ("players", serde_json::to_string(&meta.players).expect("players serialise")),
            ("rules", serde_json::to_string(&meta.rules).expect("rules serialise")),
            ("began_ms", at.to_string()),
        ];
        for (key, value) in rows {
            tx.execute(&format!("INSERT OR IGNORE INTO {s}meta (key, value) VALUES (?1, ?2)"), params![key, value]).map_err(|e| failed(&e))?;
        }
    }
    if let Some((player, action)) = action {
        tx.execute(&format!("INSERT INTO {}.actions (player, action, at_ms) VALUES (?1, ?2, ?3)", schema_of(hand)), params![player, action.to_string(), at]).map_err(|e| failed(&e))?;
    }
    for (n, event) in placed {
        tx.execute(
            &format!("INSERT INTO {}.events (kind, to_player, text, data, at_ms) VALUES (?1, ?2, ?3, ?4, ?5)", schema_of(n)),
            params![event.kind, event.to, event.text, event.data.to_string(), at],
        )
        .map_err(|e| failed(&e))?;
    }
    tx.commit().map_err(|e| failed(&e))
}

fn open(dir: &Path, hand: u32) -> Result<Connection, String> {
    Connection::open_with_flags(path(dir, hand), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| format!("hand {hand}'s database: {e}"))
}

/// The meta a table was started with, from hand-1.
pub fn meta(dir: &Path) -> Result<Meta, String> {
    let conn = open(dir, 1)?;
    let get = |key: &str| -> Result<String, String> {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get::<_, String>(0)).optional().map_err(|e| e.to_string())?.ok_or_else(|| format!("hand 1's database has no {key}"))
    };
    Ok(Meta {
        table: get("table")?,
        game: get("game")?,
        seed: get("seed")?.parse().map_err(|e| format!("seed: {e}"))?,
        players: serde_json::from_str(&get("players")?).map_err(|e| format!("players: {e}"))?,
        rules: serde_json::from_str(&get("rules")?).map_err(|e| format!("rules: {e}"))?,
    })
}

/// The table's game as its hands' databases have it: replayed from hand-1's meta with every
/// action of every hand, in order. None when the table has no hand yet.
pub fn replay(dir: &Path) -> Result<Option<Holdem>, String> {
    let last = last_hand(dir);
    if last == 0 {
        return Ok(None);
    }
    let meta = meta(dir)?;
    let (mut engine, _) = Holdem::new(&meta.players, meta.seed, meta.rules);
    for hand in 1..=last {
        let conn = open(dir, hand)?;
        let mut q = conn.prepare("SELECT player, action FROM actions ORDER BY seq").map_err(|e| e.to_string())?;
        let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
        for row in rows {
            let (player, action) = row.map_err(|e| e.to_string())?;
            let action: Value = serde_json::from_str(&action).map_err(|e| format!("hand {hand}: an action is not JSON: {e}"))?;
            if engine.to_act().as_deref() != Some(player.as_str()) {
                return Err(format!("hand {hand} does not replay: {player} acted when it was {:?}'s turn", engine.to_act()));
            }
            if action == timeout() {
                engine.timeout();
            } else {
                engine.act(&player, &action).map_err(|e| format!("hand {hand} does not replay: {player} {action}: {e}"))?;
            }
        }
    }
    Ok(Some(engine))
}

/// A hand's events in order, as `{kind, text, data}`; the public ones only, or all with `to`.
pub fn events(dir: &Path, hand: u32, public_only: bool) -> Vec<Value> {
    if hand == 0 || !path(dir, hand).exists() {
        return Vec::new();
    }
    let Ok(conn) = open(dir, hand) else { return Vec::new() };
    let sql = if public_only { "SELECT kind, to_player, text, data, seq, at_ms FROM events WHERE to_player IS NULL ORDER BY seq" } else { "SELECT kind, to_player, text, data, seq, at_ms FROM events ORDER BY seq" };
    let Ok(mut q) = conn.prepare(sql) else { return Vec::new() };
    let rows = q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get::<_, i64>(5)?)));
    let Ok(rows) = rows else { return Vec::new() };
    rows.flatten()
        .map(|(kind, to, text, data, seq, at_ms)| {
            let data: Value = serde_json::from_str(&data).unwrap_or(Value::Null);
            if public_only { json!({"seq": seq, "kind": kind, "text": text, "data": data, "at_ms": at_ms}) } else { json!({"seq": seq, "kind": kind, "to": to, "text": text, "data": data, "at_ms": at_ms}) }
        })
        .collect()
}

/// A hand's events in order as `player` may see them (CON-151, journal#1018): what happened in
/// it from the deal, the public events and its own cards, never another player's. The table's
/// start and the prompts to act stay in the database but are no part of a hand's history.
pub fn events_for(dir: &Path, hand: u32, player: &str) -> Result<Vec<Value>, String> {
    let conn = open(dir, hand)?;
    let mut q = conn.prepare("SELECT seq, kind, text, data, at_ms FROM events WHERE (to_player IS NULL OR to_player = ?1) AND kind NOT IN ('started', 'your_turn') ORDER BY seq").map_err(|e| e.to_string())?;
    let rows = q
        .query_map([player], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, i64>(4)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, kind, text, data, at_ms) = row.map_err(|e| e.to_string())?;
        out.push(json!({"seq": seq, "kind": kind, "text": text, "data": serde_json::from_str::<Value>(&data).unwrap_or(Value::Null), "at_ms": at_ms}));
    }
    Ok(out)
}
