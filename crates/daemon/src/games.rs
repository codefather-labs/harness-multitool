//! Games between agents on the daemon (ADR 0029, journal#952): the tables, who sits at them, and
//! what each agent is told. The rules are harness-games'. An agent is told by an inbound message
//! labelled `game`: when it is invited, when it is its turn (with what happened since its last
//! turn), when a hand ends and when the game does. Every message is a model turn of the agent's, so
//! what others do is gathered into its next turn's message rather than sent one by one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use harness_games::holdem::{Holdem, Rules};
use harness_games::{Event, Game};

use crate::hand_db;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The label of a game's inbound messages, drawn as a callback card of its own colour.
pub const GAME_LABEL: &str = "game";

/// The games' settings (Settings > Games, the dashboard): off until turned on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "yes")]
    pub texas_holdem: bool,
    #[serde(default)]
    pub rules: Rules,
}

fn yes() -> bool {
    true
}

impl Default for GameSettings {
    fn default() -> Self {
        GameSettings { enabled: false, texas_holdem: true, rules: Rules::default() }
    }
}

pub fn read_settings(data_dir: &Path) -> GameSettings {
    std::fs::read_to_string(data_dir.join("games").join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_settings(data_dir: &Path, settings: &GameSettings) -> std::io::Result<()> {
    let dir = data_dir.join("games");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(settings).expect("settings serialise"))
}

/// A player: a conversation, and the name it goes by at the table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub session: String,
    pub nick: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Table {
    pub id: String,
    pub game: String,
    pub invited: Vec<Player>,
    pub joined: Vec<String>,
    pub declined: Vec<String>,
    /// inviting, playing, over, cancelled.
    pub state: String,
    /// The game as its hands' databases have it (CON-149): never saved here, rebuilt from them
    /// when the daemon starts, changed only by a step written there first.
    #[serde(skip)]
    pub engine: Option<Holdem>,
    /// What happened, as told, the last 300.
    pub log: Vec<String>,
    /// What each player has not been told yet: told with their next turn.
    pub pending: BTreeMap<String, Vec<String>>,
    /// When the player to act was asked, in ms since the epoch: shown, never a deadline. A game
    /// is played at leisure and an agent may be busy, so no turn has a time limit (journal#1006).
    pub asked_ms: u64,
    pub created_ms: u64,
    pub rules: Rules,
    pub seed: u64,
}

impl Table {
    fn nick_of(&self, session: &str) -> String {
        self.invited.iter().find(|p| p.session == session).map_or_else(|| session.to_string(), |p| p.nick.clone())
    }

    fn session_of(&self, nick: &str) -> Option<String> {
        self.invited.iter().find(|p| p.nick == nick).map(|p| p.session.clone())
    }

    /// The table as the dashboard shows it: what everyone may see, no one's cards.
    pub fn public(&self) -> Value {
        json!({
            "id": self.id, "game": self.game, "state": self.state, "rules": self.rules,
            "invited": self.invited.iter().map(|p| json!({"nick": p.nick, "joined": self.joined.contains(&p.session), "declined": self.declined.contains(&p.session)})).collect::<Vec<_>>(),
            "view": self.engine.as_ref().map(|e| e.view(None)),
            "log": self.log.iter().rev().take(40).rev().collect::<Vec<_>>(),
        })
    }
}

/// What a table's game does next: a player's action, or the rules' for one who has left.
enum Step<'a> {
    Act(&'a Value),
    /// A player leaves the table, at any time (CON-154).
    Leave,
}

/// One step of a table's game (CON-149): taken on a copy, written to its hand's database with
/// what it set off, and only then made the table's. A write that fails refuses the step.
fn step(dir: &Path, table: &mut Table, player: &str, what: Step<'_>) -> Result<Vec<Event>, String> {
    let engine = table.engine.as_ref().ok_or_else(|| format!("table {} has not started", table.id))?;
    let mut next = engine.clone();
    let hand = next.hand_no();
    let (events, action) = match what {
        Step::Act(action) => (next.act(player, action)?, action.clone()),
        Step::Leave => (next.leave(player)?, hand_db::leave()),
    };
    hand_db::record(dir, &hand_db::meta(dir)?, hand, Some((player, &action)), &events)?;
    table.engine = Some(next);
    Ok(events)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The tables of a daemon.
pub struct Games {
    data_dir: PathBuf,
    tables: Mutex<BTreeMap<String, Table>>,
    /// Each table's version, raised on every change, and the signal of one (journal#1040): what the
    /// dashboard's socket waits on.
    versions: Mutex<BTreeMap<String, u64>>,
    changed: Condvar,
}

/// Whom an inviter names, among the agents online (CON-153): by nick (any case) or conversation
/// id; the inviter first; a name given twice, or the inviter's own, counted once. An unknown
/// name is refused, listing who is online.
pub fn resolve(online: &[crate::OpenSession], names: &[String], inviter: &str) -> Result<Vec<Player>, String> {
    let nick = |s: &crate::OpenSession| s.nick.clone().unwrap_or_else(|| s.title.clone());
    let me = online.iter().find(|s| s.id == inviter).ok_or("your conversation is not online")?;
    let mut players = vec![Player { session: me.id.clone(), nick: nick(me) }];
    for name in names {
        let wanted = name.trim();
        let found = online.iter().find(|s| s.id == wanted || s.nick.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(wanted)));
        let Some(s) = found else {
            let here: Vec<String> = online.iter().filter(|s| s.id != inviter).map(nick).collect();
            return Err(format!("no agent online is called {wanted:?}; online: {}", if here.is_empty() { "nobody else".to_string() } else { here.join(", ") }));
        };
        if !players.iter().any(|p| p.session == s.id) {
            players.push(Player { session: s.id.clone(), nick: nick(s) });
        }
    }
    Ok(players)
}

/// Delivers a message to a conversation as an inbound; false if it is not open.
pub type Deliver<'a> = &'a dyn Fn(&str, &str) -> bool;

impl Games {
    pub fn new(data_dir: &Path) -> Games {
        let mut tables = BTreeMap::new();
        if let Ok(files) = std::fs::read_dir(data_dir.join("games").join("tables")) {
            for file in files.flatten() {
                if let Some(mut table) = std::fs::read_to_string(file.path()).ok().and_then(|t| serde_json::from_str::<Table>(&t).ok()) {
                    // The game comes back from its hands' databases, the source of truth.
                    if matches!(table.state.as_str(), "playing" | "over") {
                        match hand_db::replay(&Self::hands_of(data_dir, &table.id)) {
                            Ok(Some(engine)) => table.engine = Some(engine),
                            Ok(None) => {
                                table.state = "damaged".into();
                                table.log.push("The table's hands' databases are missing: it cannot go on.".into());
                            }
                            Err(e) => {
                                table.state = "damaged".into();
                                table.log.push(format!("The table's hands' databases do not replay: {e}."));
                            }
                        }
                    }
                    tables.insert(table.id.clone(), table);
                }
            }
        }
        let versions = tables.keys().map(|id| (id.clone(), 1)).collect();
        Games { data_dir: data_dir.to_path_buf(), tables: Mutex::new(tables), versions: Mutex::new(versions), changed: Condvar::new() }
    }

    /// The folder of a table's hands' databases.
    fn hands_of(data_dir: &Path, table: &str) -> PathBuf {
        data_dir.join("games").join("tables").join(table)
    }

    /// The tables' versions now.
    pub fn versions(&self) -> BTreeMap<String, u64> {
        self.versions.lock().expect("versions").clone()
    }

    /// Waits until the versions differ from `seen`, or `timeout` passes; answers them as they are.
    pub fn wait_change(&self, seen: &BTreeMap<String, u64>, timeout: std::time::Duration) -> BTreeMap<String, u64> {
        let guard = self.versions.lock().expect("versions");
        let (guard, _) = self.changed.wait_timeout_while(guard, timeout, |now| now == seen).expect("versions");
        guard.clone()
    }

    fn save(&self, table: &Table) {
        {
            let mut versions = self.versions.lock().expect("versions");
            *versions.entry(table.id.clone()).or_insert(0) += 1;
        }
        self.changed.notify_all();
        let dir = self.data_dir.join("games").join("tables");
        if std::fs::create_dir_all(&dir).is_ok()
            && let Ok(text) = serde_json::to_string(table)
        {
            let partial = dir.join(format!("{}.partial", table.id));
            if std::fs::write(&partial, text).is_ok() {
                let _ = std::fs::rename(&partial, dir.join(format!("{}.json", table.id)));
            }
        }
    }

    pub fn list(&self) -> Vec<Value> {
        self.tables.lock().expect("tables").values().rev().map(Table::public).collect()
    }

    pub fn table(&self, id: &str) -> Option<Value> {
        self.tables.lock().expect("tables").get(id).map(Table::public)
    }

    /// What a watcher of the dashboard sees of a table's play (CON-152): the current hand's public
    /// events, and the previous hand's from its end, so a showdown stays on view while the next
    /// hand is dealt. Never a player's cards before they are shown.
    pub fn hand_feed(&self, id: &str) -> Option<Value> {
        if !self.tables.lock().expect("tables").contains_key(id) {
            return None;
        }
        let dir = Self::hands_of(&self.data_dir, id);
        let hand = hand_db::last_hand(&dir);
        let events: Vec<Value> = hand_db::events(&dir, hand, true).into_iter().filter(|e| e["kind"] != "started").collect();
        let previous: Vec<Value> = hand_db::events(&dir, hand.saturating_sub(1), true).into_iter().skip_while(|e| e["kind"] != "hand_ended").collect();
        Some(json!({"table": id, "hand": hand, "events": events, "previous": previous}))
    }

    /// A new table: its players are invited, each told so (ADR 0029).
    /// An agent's invitation (CON-153, journal#1049): a table with it and those it names, made at
    /// the games' settings; it is joined at once and not told of its own invitation.
    pub fn invite(&self, game: &str, players: Vec<Player>, deliver: Deliver<'_>) -> Result<(String, Vec<String>), String> {
        let inviter = players.first().ok_or("no inviter")?.clone();
        let quiet = |s: &str, text: &str| s != inviter.session && deliver(s, text);
        let id = self.create(game, players.clone(), &quiet)?;
        self.op(&inviter.session, "game_join", &json!({"table": id}), deliver)?;
        Ok((id, players.iter().skip(1).map(|p| p.nick.clone()).collect()))
    }

    pub fn create(&self, game: &str, players: Vec<Player>, deliver: Deliver<'_>) -> Result<String, String> {
        let settings = read_settings(&self.data_dir);
        if !settings.enabled {
            return Err("games are off: turn them on in Settings > Games, or on the dashboard".to_string());
        }
        if game != "texas_holdem" || !settings.texas_holdem {
            return Err(format!("no game {game:?} is on: texas_holdem is the one there is"));
        }
        if !(2..=9).contains(&players.len()) {
            return Err(format!("Texas hold'em seats 2 to 9 players, not {}", players.len()));
        }
        let mut tables = self.tables.lock().expect("tables");
        let id = format!("T{}", tables.len() + 1);
        let rules = settings.rules.clone();
        let table = Table { id: id.clone(), game: game.to_string(), invited: players.clone(), joined: Vec::new(), declined: Vec::new(), state: "inviting".into(), engine: None, log: Vec::new(), pending: BTreeMap::new(), asked_ms: 0, created_ms: now_ms(), rules: rules.clone(), seed: now_ms() };
        let names: Vec<&str> = players.iter().map(|p| p.nick.as_str()).collect();
        for p in &players {
            deliver(&p.session, &format!("You are invited to a poker table {id}: Texas hold'em with {}, stacks of {}, blinds {}/{}, play money. Join it with game_join (table {id}), or say no with game_decline. It starts when everyone has answered.", names.join(", "), rules.stack, rules.small_blind, rules.big_blind));
        }
        self.save(&table);
        tables.insert(id.clone(), table);
        Ok(id)
    }

    /// An agent's game tool, by name, for the conversation `session`.
    pub fn op(&self, session: &str, name: &str, args: &Value, deliver: Deliver<'_>) -> Result<Value, String> {
        let mut tables = self.tables.lock().expect("tables");
        if name == "game_tables" {
            let mine: Vec<Value> = tables.values().filter(|t| t.invited.iter().any(|p| p.session == session)).map(Table::public).collect();
            return Ok(json!({"tables": mine}));
        }
        let id = args["table"].as_str().ok_or("table is required: game_tables lists yours")?;
        let table = tables.get_mut(id).ok_or_else(|| format!("no table {id}"))?;
        if !table.invited.iter().any(|p| p.session == session) {
            return Err(format!("you are not at table {id}"));
        }
        let me = table.nick_of(session);
        let dir = Self::hands_of(&self.data_dir, id);
        let answer = match name {
            "game_join" | "game_decline" => {
                if table.state != "inviting" {
                    return Err(format!("table {id} is {}: it is too late to answer", table.state));
                }
                let list = if name == "game_join" { &mut table.joined } else { &mut table.declined };
                if !list.iter().any(|s| s == session) {
                    list.push(session.to_string());
                }
                table.log.push(format!("{me} {}.", if name == "game_join" { "joins" } else { "declines" }));
                Self::maybe_start(&dir, table, deliver);
                json!({"table": id, "state": table.state})
            }
            "game_state" => {
                // The hand now (CON-150), from the hands' databases, the source of truth (CON-149),
                // not the copy kept. The history is game_history's (CON-151).
                // The table is its seating and rules; its log is history, game_history's (journal#1030).
                let engine = hand_db::replay(&dir)?;
                let mut state = engine.as_ref().map_or(Value::Null, |e| e.view(Some(&me)));
                let you = state.as_object_mut().and_then(|s| s.remove("you")).unwrap_or(Value::Null);
                let seating = table.public();
                let table_now = json!({"id": seating["id"], "game": seating["game"], "state": seating["state"], "rules": seating["rules"], "invited": seating["invited"]});
                json!({"table": table_now, "state": state, "you": you})
            }
            "game_history" => {
                // The current hand's events, from its own database; earlier hands are no agent's
                // (CON-151, journal#1018).
                let hand = hand_db::last_hand(&dir);
                if hand == 0 {
                    return Err(format!("table {id} has not started: no hand is being played"));
                }
                let events = hand_db::events_for(&dir, hand, &me)?;
                let ended = events.iter().any(|e| e["kind"] == "hand_ended");
                json!({"table": id, "hand": hand, "ended": ended, "events": events})
            }
            "game_act" => {
                let events = step(&dir, table, &me, Step::Act(args))?;
                Self::tell(table, events, deliver);
                json!({"ok": true})
            }
            "game_leave" => {
                // Leaving takes the player off the table, whenever it wants (CON-154).
                match table.state.as_str() {
                    "inviting" => {
                        if !table.declined.iter().any(|s| s == session) {
                            table.declined.push(session.to_string());
                            table.joined.retain(|s| s != session);
                            table.log.push(format!("{me} declines."));
                        }
                        Self::maybe_start(&dir, table, deliver);
                        json!({"left": id, "declined": true})
                    }
                    "playing" => {
                        let events = step(&dir, table, &me, Step::Leave)?;
                        table.declined.push(session.to_string());
                        let stack = events.first().and_then(|e| e.data["stack"].as_u64()).unwrap_or(0);
                        Self::tell(table, events, deliver);
                        json!({"left": id, "stack": stack, "state": table.state})
                    }
                    state => return Err(format!("table {id} is {state}: there is nothing to leave")),
                }
            }
            other => return Err(format!("no game tool {other}")),
        };
        let snapshot = table.clone();
        drop(tables);
        self.save(&snapshot);
        Ok(answer)
    }

    /// Everyone has answered: two or more who joined play; fewer, and it is called off.
    fn maybe_start(dir: &Path, table: &mut Table, deliver: Deliver<'_>) {
        if table.joined.len() + table.declined.len() < table.invited.len() {
            return;
        }
        let players: Vec<String> = table.invited.iter().filter(|p| table.joined.contains(&p.session)).map(|p| p.nick.clone()).collect();
        if players.len() < 2 {
            table.state = "cancelled".into();
            for s in &table.joined {
                deliver(s, &format!("Table {} is called off: not enough players joined.", table.id));
            }
            return;
        }
        let (engine, events) = Holdem::new(&players, table.seed, table.rules.clone());
        let meta = hand_db::Meta { table: table.id.clone(), game: table.game.clone(), seed: table.seed, players, rules: table.rules.clone() };
        if let Err(e) = hand_db::record(dir, &meta, 1, None, &events) {
            // Not started: the next answer to the table tries again.
            table.log.push(format!("The table could not start: {e}."));
            return;
        }
        table.engine = Some(engine);
        table.state = "playing".into();
        Self::tell(table, events, deliver);
    }

    /// What the rules said goes to the players: a turn to its player with what they have not heard
    /// yet; the end of a hand and of the game to all; the rest gathered for each one's next turn.
    fn tell(table: &mut Table, events: Vec<Event>, deliver: Deliver<'_>) {
        let id = table.id.clone();
        for event in events {
            // The log is everyone's: a player's own cards and turn are theirs alone.
            if event.to.is_none() {
                table.log.push(event.text.clone());
            }
            let everyone: Vec<String> = table.invited.iter().filter(|p| table.joined.contains(&p.session)).map(|p| p.session.clone()).collect();
            match (event.kind.as_str(), &event.to) {
                ("your_turn", Some(nick)) => {
                    let Some(session) = table.session_of(nick) else { continue };
                    if table.declined.contains(&session) {
                        continue;
                    }
                    let before = table.pending.remove(&session).unwrap_or_default();
                    let since = if before.is_empty() { String::new() } else { format!("Since your last turn: {}\n\n", before.join(" ")) };
                    deliver(&session, &format!("[table {id}] {since}{}", event.text));
                    table.asked_ms = now_ms();
                }
                ("cards", _) | ("your_turn", None) => {}
                ("hand_ended" | "game_over", _) => {
                    for s in &everyone {
                        let before = table.pending.remove(s).unwrap_or_default();
                        let since = if before.is_empty() { String::new() } else { format!("{} ", before.join(" ")) };
                        if !table.declined.contains(s) {
                            deliver(s, &format!("[table {id}] {since}{}", event.text));
                        }
                    }
                    if event.kind == "game_over" {
                        table.state = "over".into();
                    }
                }
                _ => {
                    for s in &everyone {
                        table.pending.entry(s.clone()).or_default().push(event.text.clone());
                    }
                }
            }
        }
        let keep = table.log.len().saturating_sub(300);
        table.log.drain(..keep);
    }

    /// A player that left in a build before CON-154 (kept as declined, still seated) is taken off
    /// the table now, as game_leave does. No one is ever acted for otherwise: a turn waits for its
    /// player however long it takes (journal#1006).
    pub fn tick(&self, deliver: Deliver<'_>) {
        let mut changed = Vec::new();
        {
            let mut tables = self.tables.lock().expect("tables");
            for table in tables.values_mut().filter(|t| t.state == "playing") {
                let Some(view) = table.engine.as_ref().map(|e| e.view(None)) else { continue };
                let seated = |nick: &str| view["players"].as_array().into_iter().flatten().any(|p| p["player"] == nick && p["left"] == false && p["out"] == false);
                let gone = table.invited.iter().find(|p| table.declined.contains(&p.session) && seated(&p.nick)).map(|p| p.nick.clone());
                if let Some(nick) = gone {
                    match step(&Self::hands_of(&self.data_dir, &table.id), table, &nick, Step::Leave) {
                        Ok(events) => {
                            Self::tell(table, events, deliver);
                            changed.push(table.clone());
                        }
                        // Tried again at the next tick; nothing was taken.
                        Err(e) => eprintln!("multitool daemon: table {}: {e}", table.id),
                    }
                }
            }
        }
        for table in changed {
            self.save(&table);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn players() -> Vec<Player> {
        vec![Player { session: "s-ann".into(), nick: "ann".into() }, Player { session: "s-bob".into(), nick: "bob".into() }]
    }

    /// ADR 0029: invited, joined, started; each agent told its turn and only what concerns it, what
    /// others did gathered into its next turn; illegal moves refused; a silent one acted for.
    #[test]
    fn a_table_invites_starts_and_tells_each_agent_its_turn() {
        let dir = std::env::temp_dir().join(format!("mt-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let said: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let deliver = |s: &str, text: &str| {
            said.borrow_mut().push((s.to_string(), text.to_string()));
            true
        };
        let games = Games::new(&dir);
        assert!(games.create("texas_holdem", players(), &deliver).unwrap_err().contains("games are off"));
        write_settings(&dir, &GameSettings { enabled: true, ..Default::default() }).unwrap();
        let id = games.create("texas_holdem", players(), &deliver).unwrap();
        assert_eq!(said.borrow().iter().filter(|(_, t)| t.contains("You are invited to a poker table T1")).count(), 2);
        assert!(games.op("s-eve", "game_join", &json!({"table": id}), &deliver).unwrap_err().contains("not at table"));
        assert!(games.op("s-ann", "game_act", &json!({"table": id, "do": "call"}), &deliver).unwrap_err().contains("has not started"));
        games.op("s-ann", "game_join", &json!({"table": id}), &deliver).unwrap();
        said.borrow_mut().clear();
        games.op("s-bob", "game_join", &json!({"table": id}), &deliver).unwrap();
        // Heads-up the button, ann, acts first: she alone is told, with her cards.
        let told = said.borrow().clone();
        assert_eq!(told.len(), 1, "{told:?}");
        assert_eq!(told[0].0, "s-ann");
        assert!(told[0].1.contains("Your turn") && told[0].1.contains("you hold"), "{}", told[0].1);
        let ann_view = games.op("s-ann", "game_state", &json!({"table": id}), &deliver).unwrap();
        let bob_view = games.op("s-bob", "game_state", &json!({"table": id}), &deliver).unwrap();
        assert_ne!(ann_view["you"]["cards"], bob_view["you"]["cards"], "each sees their own cards");
        assert!(games.op("s-bob", "game_act", &json!({"table": id, "do": "check"}), &deliver).unwrap_err().contains("not your turn"));
        said.borrow_mut().clear();
        games.op("s-ann", "game_act", &json!({"table": id, "do": "call"}), &deliver).unwrap();
        let told = said.borrow().clone();
        assert_eq!(told.len(), 1, "{told:?}");
        assert!(told[0].0 == "s-bob" && told[0].1.contains("Since your last turn: ") && told[0].1.contains("ann calls 10.") && told[0].1.contains("Your turn"), "{}", told[0].1);
        // Bob is silent: no time limit, the rules never act for him while he sits (journal#1006).
        said.borrow_mut().clear();
        if let Some(t) = games.tables.lock().unwrap().get_mut(&id) {
            t.asked_ms = 0;
        }
        games.tick(&deliver);
        assert!(said.borrow().is_empty(), "a silent player is waited for: {:?}", said.borrow());
        // game_history gives the hand so far, in order, from its database (CON-151): the public
        // events and the asker's own cards, never another's.
        let hands_dir = dir.join("games/tables").join(&id);
        let history = |session: &str| games.op(session, "game_history", &json!({"table": id}), &deliver);
        let kinds = |h: &Value| h["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        let bob_history = history("s-bob").unwrap();
        assert_eq!(kinds(&bob_history), ["hand", "cards", "acted"], "from the deal; no table start, no prompts: {bob_history}");
        assert_eq!((bob_history["hand"].as_u64(), bob_history["ended"].as_bool()), (Some(1), Some(false)));
        assert!(bob_history["events"][2]["text"].as_str().unwrap().contains("ann calls 10"), "{bob_history}");
        let seqs: Vec<i64> = bob_history["events"].as_array().unwrap().iter().map(|e| e["seq"].as_i64().unwrap()).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]) && bob_history["events"][0]["at_ms"].as_i64().unwrap() > 0, "{seqs:?}");
        let ann_cards = ann_view["you"]["cards"].as_str().unwrap().to_string();
        let bob_cards = bob_view["you"]["cards"].as_str().unwrap().to_string();
        assert!(bob_history.to_string().contains(&bob_cards) && !bob_history.to_string().contains(&ann_cards), "only his own cards: {bob_history}");
        // game_state is the hand now (CON-150), with no history in it.
        let now = games.op("s-bob", "game_state", &json!({"table": id}), &deliver).unwrap();
        assert!(now.get("hand").is_none() && now.get("last_hand").is_none(), "{now}");
        // journal#1030: no `you` left in state, and the table is its seating and rules only.
        assert!(now["state"].get("you").is_none(), "{now}");
        assert!(now["table"].get("view").is_none() && now["table"].get("log").is_none(), "{now}");
        assert_eq!(now["table"]["id"], id.as_str());
        assert!(now["table"]["invited"].is_array() && now["table"]["rules"].is_object());
        assert_eq!((now["state"]["still_in_hand"].as_u64(), now["state"]["pot"].as_u64(), now["you"]["in_hand"].as_u64()), (Some(2), Some(40), Some(20)), "{now}");
        assert!(now["state"]["players"][0]["in_hand"].is_u64() && now["you"]["cards"] == bob_view["you"]["cards"]);
        games.op("s-bob", "game_act", &json!({"table": id, "do": "check"}), &deliver).unwrap();
        // After the flop, heads-up, the big blind (bob) acts first: he is told, with what happened.
        assert!(said.borrow().iter().any(|(s, t)| s == "s-bob" && t.contains("bob checks") && t.contains("the flop")), "{:?}", said.borrow());
        // The dashboard, and any agent through game_state, sees no one's cards: not in the view,
        // not in the log.
        let public = games.table(&id).unwrap();
        assert!(public["view"]["you"].is_null() && public["state"] == "playing");
        let log = public["log"].to_string();
        assert!(!log.contains("your cards") && !log.contains("you hold"), "a private message in the public log: {log}");
        assert!(!bob_view["table"]["log"].to_string().contains("you hold"));
        assert_eq!(kinds(&history("s-ann").unwrap()), ["hand", "cards", "acted", "acted", "board"]);
        // Bob checks, ann bets and bob folds: the hand ends, and the finished hand stays readable
        // whole while the next one is dealt.
        games.op("s-bob", "game_act", &json!({"table": id, "do": "check"}), &deliver).unwrap();
        games.op("s-ann", "game_act", &json!({"table": id, "do": "bet", "amount": 40}), &deliver).unwrap();
        games.op("s-bob", "game_act", &json!({"table": id, "do": "fold"}), &deliver).unwrap();
        // Only the current hand: hand 1 ended, and the tools now give hand 2 from its own database.
        let current = history("s-ann").unwrap();
        assert_eq!((current["hand"].as_u64(), current["ended"].as_bool()), (Some(2), Some(false)));
        assert_eq!(current["events"][0]["kind"], "hand", "the next hand, from its deal: {current}");
        assert!(!current.to_string().contains("Hand 1"), "nothing of the hand before: {current}");
        // The dashboard's feed (CON-152): the current hand's public events with their seq, and the
        // previous hand from its end; no one's cards.
        let feed = games.hand_feed(&id).unwrap();
        assert_eq!((feed["hand"].as_u64(), feed["events"][0]["kind"].as_str()), (Some(2), Some("hand")), "{feed}");
        assert!(feed["events"][0]["seq"].is_i64());
        assert_eq!(feed["previous"][0]["kind"], "hand_ended", "{feed}");
        assert!(!feed.to_string().contains("\"cards\""), "no one's cards: {feed}");
        assert!(games.hand_feed("T9").is_none());
        assert_eq!(hand_db::events(&hands_dir, 1, true).last().unwrap()["kind"], "hand_ended", "hand 1 is whole in its database");
        let state = games.op("s-ann", "game_state", &json!({"table": id}), &deliver).unwrap();
        // Every hand has its database, the source of truth (CON-149): hand 1 whole with its
        // actions and its private events too, hand 2 begun; replayed, they are the live game.
        let hands = hands_dir.clone();
        assert!(hands.join("hand-1.sqlite").exists() && hands.join("hand-2.sqlite").exists());
        let all = hand_db::events(&hands, 1, false);
        assert!(all.iter().any(|e| e["kind"] == "cards" && e["to"] == "ann"), "private events are kept: {all:?}");
        assert_eq!(all.first().unwrap()["kind"], "started");
        let conn = rusqlite::Connection::open(hands.join("hand-1.sqlite")).unwrap();
        let acts: Vec<(String, String)> = conn.prepare("SELECT player, action FROM actions ORDER BY seq").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(acts.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["ann", "bob", "bob", "ann", "bob"], "{acts:?}");
        assert_eq!(acts[4].1, json!({"do": "fold", "table": id}).to_string(), "each action as given");
        let live = games.tables.lock().unwrap()[&id].engine.as_ref().unwrap().view(None);
        assert_eq!(hand_db::replay(&hands).unwrap().unwrap().view(None), live, "the databases replay to the live game");
        let mut live_no_you = live.clone();
        live_no_you.as_object_mut().unwrap().remove("you");
        assert_eq!(state["state"], live_no_you, "game_state answers from the databases");
        // A step that cannot be written is refused, and nothing changes.
        use std::os::unix::fs::PermissionsExt;
        let to_act = live["to_act"].as_str().unwrap().to_string();
        let session = if to_act == "ann" { "s-ann" } else { "s-bob" };
        std::fs::set_permissions(&hands, std::fs::Permissions::from_mode(0o555)).unwrap();
        let refused = games.op(session, "game_act", &json!({"table": id, "do": "fold"}), &deliver);
        std::fs::set_permissions(&hands, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(refused.as_ref().is_err_and(|e| e.contains("could not be written")), "{refused:?}");
        assert_eq!(games.tables.lock().unwrap()[&id].engine.as_ref().unwrap().view(None), live, "a refused step changes nothing");
        // A table is kept on disk, and comes back with the daemon: its game from the databases.
        let again = Games::new(&dir);
        assert_eq!(again.table(&id).unwrap()["state"], "playing");
        assert_eq!(again.table(&id).unwrap()["view"], live, "the engine is rebuilt from the databases");
        assert!(!std::fs::read_to_string(dir.join("games/tables").join(format!("{id}.json"))).unwrap().contains("\"engine\""), "the engine is not kept in the table's file");
        // For a picture of the dashboard: the table's file, kept where asked.
        if let Some(keep) = std::env::var_os("MT_KEEP_GAMES") {
            let _ = std::fs::copy(dir.join("games/tables/T1.json"), keep);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CON-154: a player leaves whenever it wants and is taken off the table: out of the hand at
    /// once, never asked again, out of the game at the hand's end; recorded, and replayed.
    #[test]
    fn leaving_takes_a_player_off_the_table() {
        let dir = std::env::temp_dir().join(format!("mt-games-leave-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_settings(&dir, &GameSettings { enabled: true, ..Default::default() }).unwrap();
        let said: RefCell<Vec<(String, String)>> = RefCell::new(Vec::new());
        let deliver = |s: &str, text: &str| {
            said.borrow_mut().push((s.to_string(), text.to_string()));
            true
        };
        let games = Games::new(&dir);
        let three = vec![Player { session: "s-ann".into(), nick: "ann".into() }, Player { session: "s-bob".into(), nick: "bob".into() }, Player { session: "s-cat".into(), nick: "cat".into() }];
        // Leaving a table still inviting declines it.
        let early = games.create("texas_holdem", three.clone(), &deliver).unwrap();
        assert_eq!(games.op("s-cat", "game_leave", &json!({"table": early}), &deliver).unwrap()["declined"], true);
        let id = games.create("texas_holdem", three.clone(), &deliver).unwrap();
        for p in &three {
            games.op(&p.session, "game_join", &json!({"table": id}), &deliver).unwrap();
        }
        // Ann (the button) is to act; cat, the big blind, leaves out of turn.
        said.borrow_mut().clear();
        let left = games.op("s-cat", "game_leave", &json!({"table": id}), &deliver).unwrap();
        assert_eq!(left["stack"], 980, "{left}");
        assert!(said.borrow().is_empty(), "no one prompted again: {:?}", said.borrow());
        assert!(games.op("s-cat", "game_leave", &json!({"table": id}), &deliver).unwrap_err().contains("left this table already"));
        games.op("s-ann", "game_act", &json!({"table": id, "do": "call"}), &deliver).unwrap();
        games.op("s-bob", "game_act", &json!({"table": id, "do": "call"}), &deliver).unwrap();
        let told = said.borrow().clone();
        assert!(told.iter().any(|(s, t)| s == "s-bob" && t.contains("cat leaves the table with 980 chips")) || told.iter().any(|(s, t)| s == "s-ann" && t.contains("cat leaves the table")), "the others are told: {told:?}");
        assert!(!told.iter().any(|(s, _)| s == "s-cat"), "the one who left hears nothing more: {told:?}");
        // Recorded as cat's own action, out of turn, and replayed.
        let hands = dir.join("games/tables").join(&id);
        let conn = rusqlite::Connection::open(hands.join("hand-1.sqlite")).unwrap();
        let acts: Vec<(String, String)> = conn.prepare("SELECT player, action FROM actions ORDER BY seq").unwrap().query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect();
        assert_eq!(acts[0], ("cat".to_string(), hand_db::leave().to_string()), "{acts:?}");
        let live = games.tables.lock().unwrap()[&id].engine.as_ref().unwrap().view(None);
        assert_eq!(hand_db::replay(&hands).unwrap().unwrap().view(None), live, "the leave replays");
        assert_eq!(live["street"], "flop", "the round closed without the one who left: {live}");
        // Ann and bob play the hand out: in the next one cat is out, dealt nothing.
        let to = live["to_act"].as_str().unwrap().to_string();
        games.op(&format!("s-{to}"), "game_act", &json!({"table": id, "do": "bet", "amount": 20}), &deliver).unwrap();
        let other = if to == "ann" { "bob" } else { "ann" };
        games.op(&format!("s-{other}"), "game_act", &json!({"table": id, "do": "fold"}), &deliver).unwrap();
        let v = games.table(&id).unwrap()["view"].clone();
        assert_eq!((v["hand"].as_u64(), v["still_in_game"].as_u64()), (Some(2), Some(2)), "{v}");
        assert_eq!(v["players"][2]["out"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// CON-153: an inviter names agents online by nick (any case) or id; itself first, each once;
    /// an unknown name is refused, listing who is online.
    #[test]
    fn an_invitation_names_agents_online() {
        let online = |id: &str, nick: Option<&str>| crate::OpenSession { id: id.into(), title: format!("{id} title"), nick: nick.map(str::to_string) };
        let all = vec![online("c-me", Some("multitool-arch")), online("c-tt", Some("tacticstrade-arch")), online("c-x", None)];
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let got = resolve(&all, &names(&["TacticsTrade-Arch", "c-x", "multitool-arch", "tacticstrade-arch"]), "c-me").unwrap();
        assert_eq!(got.iter().map(|p| (p.session.as_str(), p.nick.as_str())).collect::<Vec<_>>(), [("c-me", "multitool-arch"), ("c-tt", "tacticstrade-arch"), ("c-x", "c-x title")]);
        let err = resolve(&all, &names(&["nobody"]), "c-me").unwrap_err();
        assert!(err.contains("\"nobody\"") && err.contains("tacticstrade-arch") && !err.contains("multitool-arch"), "{err}");
        // Invited: the inviter is joined at once and not told of its own invitation.
        let dir = std::env::temp_dir().join(format!("mt-games-invite-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_settings(&dir, &GameSettings { enabled: true, ..Default::default() }).unwrap();
        let said: RefCell<Vec<String>> = RefCell::new(Vec::new());
        let deliver = |s: &str, _: &str| {
            said.borrow_mut().push(s.to_string());
            true
        };
        let games = Games::new(&dir);
        let players = resolve(&all, &names(&["tacticstrade-arch"]), "c-me").unwrap();
        let (id, invited) = games.invite("texas_holdem", players, &deliver).unwrap();
        assert_eq!(invited, ["tacticstrade-arch"]);
        assert_eq!(*said.borrow(), ["c-tt"], "only the invited are told");
        let t = games.table(&id).unwrap();
        assert_eq!((t["invited"][0]["joined"].as_bool(), t["invited"][1]["joined"].as_bool()), (Some(true), Some(false)));
        // One alone is no table.
        assert!(games.invite("texas_holdem", resolve(&all, &[], "c-me").unwrap(), &deliver).unwrap_err().contains("2 to 9"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// For a look at the dashboard (CON-152): with MT_DASHBOARD_FIXTURE set to a daemon's data
    /// folder, makes tables there in the states the table view draws, and keeps them. Skipped
    /// otherwise.
    #[test]
    fn dashboard_fixture() {
        let Some(dir) = std::env::var_os("MT_DASHBOARD_FIXTURE").map(std::path::PathBuf::from) else { return };
        let deliver = |_: &str, _: &str| true;
        write_settings(&dir, &GameSettings { enabled: true, ..Default::default() }).unwrap();
        let games = Games::new(&dir);
        let seat = |names: &[&str]| names.iter().map(|n| Player { session: format!("s-{n}"), nick: n.to_string() }).collect::<Vec<_>>();
        let start = |players: Vec<Player>| {
            let id = games.create("texas_holdem", players.clone(), &deliver).unwrap();
            for p in &players {
                games.op(&p.session, "game_join", &json!({"table": id}), &deliver).unwrap();
            }
            id
        };
        let act = |id: &str, what: Value| {
            let state = games.table(id).unwrap();
            let to = state["view"]["to_act"].as_str().unwrap().to_string();
            let mut args = what;
            args["table"] = json!(id);
            games.op(&format!("s-{to}"), "game_act", &args, &deliver).unwrap();
        };
        // T1: two players on the flop, a bet in front of one.
        let t1 = start(seat(&["multitool-arch", "tacticstrade-arch"]));
        act(&t1, json!({"do": "raise", "amount": 60}));
        act(&t1, json!({"do": "call"}));
        act(&t1, json!({"do": "check"}));
        act(&t1, json!({"do": "bet", "amount": 80}));
        // T2: six players preflop, a raise and a fold.
        let t2 = start(seat(&["ann", "bob", "carol", "dave", "eve", "frank"]));
        act(&t2, json!({"do": "raise", "amount": 60}));
        act(&t2, json!({"do": "fold"}));
        act(&t2, json!({"do": "call"}));
        // T3: two players checked down to a showdown; the next hand is dealt.
        let t3 = start(seat(&["north", "south"]));
        act(&t3, json!({"do": "call"}));
        for _ in 0..7 {
            act(&t3, json!({"do": "check"}));
        }
        println!("tables {t1} {t2} {t3}");
    }

    #[test]
    fn a_table_too_few_join_is_called_off() {
        let dir = std::env::temp_dir().join(format!("mt-games-off-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_settings(&dir, &GameSettings { enabled: true, ..Default::default() }).unwrap();
        let deliver = |_: &str, _: &str| true;
        let games = Games::new(&dir);
        let id = games.create("texas_holdem", players(), &deliver).unwrap();
        games.op("s-ann", "game_join", &json!({"table": id}), &deliver).unwrap();
        games.op("s-bob", "game_decline", &json!({"table": id}), &deliver).unwrap();
        assert_eq!(games.table(&id).unwrap()["state"], "cancelled");
        assert!(games.create("chess", players(), &deliver).unwrap_err().contains("texas_holdem"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
