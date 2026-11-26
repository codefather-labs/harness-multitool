//! Group chats of agents, 'rooms' (CON-203, journal#1694). An orchestrator opens one with agents
//! of its orchestra; every message goes to every member but its author, and one member can be
//! answered in the open. The daemon holds the rooms; this module is their rules, with no I/O but
//! a room's log file, so the rules are tested apart from the daemon.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

/// At most this many messages in a room.
pub const MAX_MESSAGES: usize = 200;
/// At most this many posts by one member in [`POST_WINDOW_MS`].
pub const MAX_POSTS: usize = 20;
pub const POST_WINDOW_MS: u64 = 10 * 60 * 1000;
/// A room with no post for this long closes by itself.
pub const IDLE_MS: u64 = 30 * 60 * 1000;
/// The author of what the user writes in the room's tab.
pub const USER: &str = "user";

/// What a member is told when it joins: how to take part (point 5).
pub const MEMBER_RULES: &str = "You are a member of a room: a group chat of agents, opened by your orchestrator for working something out together. Every message in it reaches every member. Post with room_post {text, reply_to} when you add something: an idea, an observation, an objection, a check. Silence is fine; you need not answer every message, but answer what is addressed to you ('→ you'). reply_to answers one member by nick or message number, and everyone still sees it. Keep messages short; put work and evidence in the journal and cite journal#N. room_read gives the room so far.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub session: String,
    pub nick: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Message {
    pub n: u64,
    /// The author's session, or [`USER`].
    pub author: String,
    pub author_nick: String,
    /// The member it answers, by session.
    pub to: Option<String>,
    pub to_nick: Option<String>,
    pub text: String,
    pub at_ms: u64,
}

/// One message for one member, as the daemon delivers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub session: String,
    pub label: String,
    pub text: String,
    /// The user's words, delivered as a prompt; else data from outside.
    pub as_user: bool,
}

#[derive(Debug)]
pub struct Room {
    pub id: String,
    pub topic: String,
    pub goal: String,
    /// The orchestrator that opened it.
    pub owner: String,
    pub members: Vec<Member>,
    pub messages: Vec<Message>,
    pub open: bool,
    pub last_ms: u64,
    posts: HashMap<String, VecDeque<u64>>,
    log: Option<PathBuf>,
}

impl Room {
    fn nick(&self, session: &str) -> String {
        if session == USER {
            return USER.to_string();
        }
        self.members.iter().find(|m| m.session == session).map_or_else(|| session.to_string(), |m| m.nick.clone())
    }

    fn write(&self, line: Value) {
        if let Some(path) = &self.log {
            use std::io::Write;
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = writeln!(file, "{line}");
            }
        }
    }

    /// Who is in it, as the room's tools say it.
    pub fn summary(&self) -> Value {
        json!({"room": self.id, "topic": self.topic, "goal": self.goal, "owner": self.nick(&self.owner), "members": self.members.iter().map(|m| m.nick.clone()).collect::<Vec<_>>(), "open": self.open, "messages": self.messages.len()})
    }
}

/// Every room of a daemon.
#[derive(Debug, Default)]
pub struct Rooms {
    pub rooms: Vec<Room>,
    dir: Option<PathBuf>,
    next: u64,
}

impl Rooms {
    /// Rooms whose logs go under `dir` (`<data dir>/rooms`).
    pub fn new(dir: Option<&Path>) -> Rooms {
        Rooms { rooms: Vec::new(), dir: dir.map(Path::to_path_buf), next: 1 }
    }

    /// Opens a room. `members` are the orchestrator's agents; the owner is a member too.
    pub fn open(&mut self, owner: Member, topic: &str, goal: &str, members: Vec<Member>, now_ms: u64) -> Result<&Room, String> {
        let topic = topic.trim();
        if topic.is_empty() {
            return Err("topic is required".to_string());
        }
        if members.is_empty() {
            return Err("members is required: the nicks of your agents to invite".to_string());
        }
        let id = format!("r{}-{}", now_ms, self.next);
        self.next += 1;
        let mut all = vec![owner.clone()];
        for m in members {
            if !all.iter().any(|a| a.session == m.session) {
                all.push(m);
            }
        }
        let log = self.dir.as_ref().map(|d| d.join(format!("{id}.jsonl")));
        let room = Room { id, topic: topic.to_string(), goal: goal.trim().to_string(), owner: owner.session, members: all, messages: Vec::new(), open: true, last_ms: now_ms, posts: HashMap::new(), log };
        room.write(json!({"open": room.summary(), "at_ms": now_ms}));
        self.rooms.push(room);
        Ok(self.rooms.last().expect("pushed"))
    }

    /// The open rooms `session` is in.
    pub fn of(&self, session: &str) -> Vec<&Room> {
        self.rooms.iter().filter(|r| r.open && r.members.iter().any(|m| m.session == session)).collect()
    }

    /// The room a call means: the one named, else the only open room it is in.
    fn pick(&mut self, session: &str, room: Option<&str>) -> Result<&mut Room, String> {
        if let Some(id) = room.filter(|id| !id.is_empty()) {
            let found = self.rooms.iter_mut().find(|r| r.id == id).ok_or(format!("no room {id}"))?;
            if session != USER && !found.members.iter().any(|m| m.session == session) {
                return Err(format!("you are not a member of {id}"));
            }
            return Ok(found);
        }
        let mine: Vec<usize> = self.rooms.iter().enumerate().filter(|(_, r)| r.open && (session == USER || r.members.iter().any(|m| m.session == session))).map(|(i, _)| i).collect();
        match mine.as_slice() {
            [] => Err("you are in no open room".to_string()),
            [one] => Ok(&mut self.rooms[*one]),
            _ => Err(format!("you are in several rooms: say which with room ({})", mine.iter().map(|i| self.rooms[*i].id.clone()).collect::<Vec<_>>().join(", "))),
        }
    }

    /// A post by `author` (a session, or [`USER`]): numbered, logged, and what each other member
    /// gets.
    pub fn post(&mut self, author: &str, room: Option<&str>, text: &str, reply_to: Option<&str>, now_ms: u64) -> Result<(String, Message, Vec<Delivery>), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("text is required".to_string());
        }
        let r = self.pick(author, room)?;
        if !r.open {
            return Err(format!("room {} is closed", r.id));
        }
        if r.messages.len() >= MAX_MESSAGES {
            return Err(format!("room {} holds its {MAX_MESSAGES} messages: the orchestrator closes it and opens another if needed", r.id));
        }
        if author != USER {
            let times = r.posts.entry(author.to_string()).or_default();
            while times.front().is_some_and(|at| now_ms.saturating_sub(*at) > POST_WINDOW_MS) {
                times.pop_front();
            }
            if times.len() >= MAX_POSTS {
                return Err(format!("{MAX_POSTS} posts in 10 minutes is a member's limit: wait, and let the others speak"));
            }
            times.push_back(now_ms);
        }
        // reply_to: a member's nick, or the number of a message, whose author is answered.
        let to = match reply_to.map(str::trim).filter(|t| !t.is_empty()) {
            None => None,
            Some(target) => {
                let by_number = target.trim_start_matches('#').parse::<u64>().ok().and_then(|n| r.messages.iter().find(|m| m.n == n)).map(|m| m.author.clone());
                let by_nick = r.members.iter().find(|m| m.nick.eq_ignore_ascii_case(target.trim_start_matches('@')) || m.session == target).map(|m| m.session.clone());
                let user = target.eq_ignore_ascii_case(USER).then(|| USER.to_string());
                Some(by_number.or(by_nick).or(user).ok_or(format!("no member or message {target} in this room"))?)
            }
        };
        let message = Message { n: r.messages.len() as u64 + 1, author: author.to_string(), author_nick: r.nick(author), to_nick: to.as_deref().map(|t| r.nick(t)), to, text: text.to_string(), at_ms: now_ms };
        r.messages.push(message.clone());
        r.last_ms = now_ms;
        r.write(json!({"message": message}));
        let deliveries = r
            .members
            .iter()
            .filter(|m| m.session != author)
            .map(|m| {
                let to_you = message.to.as_deref() == Some(m.session.as_str());
                let addressed = match (&message.to_nick, to_you) {
                    (_, true) => " → you".to_string(),
                    (Some(nick), false) => format!(" → {nick}"),
                    _ => String::new(),
                };
                if author == USER {
                    Delivery { session: m.session.clone(), label: String::new(), text: format!("[room {} #{}, the user{addressed}] {}", r.topic, message.n, message.text), as_user: true }
                } else {
                    Delivery { session: m.session.clone(), label: format!("room:{} #{} {}{addressed}", r.topic, message.n, message.author_nick), text: message.text.clone(), as_user: false }
                }
            })
            .collect();
        Ok((r.id.clone(), message, deliveries))
    }

    /// The room's messages after `since`.
    pub fn read(&mut self, session: &str, room: Option<&str>, since: u64) -> Result<Value, String> {
        let r = self.pick(session, room)?;
        Ok(json!({"room": r.summary(), "messages": r.messages.iter().filter(|m| m.n > since).collect::<Vec<_>>()}))
    }

    /// Closes a room its owner (or the user) ends; answers it and who is to be told.
    pub fn close(&mut self, by: &str, room: Option<&str>, summary: &str, now_ms: u64) -> Result<(String, String, Vec<Member>), String> {
        let r = self.pick(by, room)?;
        if by != USER && r.owner != by {
            return Err("only the orchestrator that opened the room closes it".to_string());
        }
        if !r.open {
            return Err(format!("room {} is closed already", r.id));
        }
        r.open = false;
        r.write(json!({"close": {"by": r.nick(by), "summary": summary}, "at_ms": now_ms}));
        Ok((r.id.clone(), r.topic.clone(), r.members.clone()))
    }

    /// Rooms with no post for `idle_ms`, closed now: (id, topic, members).
    pub fn close_idle(&mut self, now_ms: u64, idle_ms: u64) -> Vec<(String, String, Vec<Member>)> {
        let mut closed = Vec::new();
        for r in self.rooms.iter_mut().filter(|r| r.open && now_ms.saturating_sub(r.last_ms) >= idle_ms) {
            r.open = false;
            r.write(json!({"close": {"by": "idle", "summary": ""}, "at_ms": now_ms}));
            closed.push((r.id.clone(), r.topic.clone(), r.members.clone()));
        }
        closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(s: &str) -> Member {
        Member { session: format!("s-{s}"), nick: s.to_string() }
    }

    fn room() -> (Rooms, String) {
        let mut rooms = Rooms::new(None);
        let id = rooms.open(member("lead"), "cache design", "pick a cache", vec![member("ana"), member("bob"), member("cid")], 1_000).unwrap().id.clone();
        (rooms, id)
    }

    #[test]
    fn a_post_reaches_every_member_but_its_author() {
        let (mut rooms, id) = room();
        let (room_id, m, out) = rooms.post("s-ana", None, "LRU is enough", None, 2_000).unwrap();
        assert_eq!((room_id, m.n, m.author_nick.as_str()), (id, 1, "ana"));
        let to: Vec<&str> = out.iter().map(|d| d.session.as_str()).collect();
        assert_eq!(to, ["s-lead", "s-bob", "s-cid"]);
        assert!(out.iter().all(|d| d.label == "room:cache design #1 ana" && !d.as_user && d.text == "LRU is enough"));
    }

    #[test]
    fn a_reply_is_seen_by_all_and_marked_for_its_addressee() {
        let (mut rooms, _) = room();
        rooms.post("s-ana", None, "LRU is enough", None, 2_000).unwrap();
        let (_, m, out) = rooms.post("s-bob", None, "not under scans", Some("#1"), 3_000).unwrap();
        assert_eq!(m.to.as_deref(), Some("s-ana"));
        let label = |s: &str| out.iter().find(|d| d.session == s).unwrap().label.clone();
        assert_eq!(label("s-ana"), "room:cache design #2 bob → you");
        assert_eq!(label("s-cid"), "room:cache design #2 bob → ana");
        let (_, m, _) = rooms.post("s-cid", None, "agree with bob", Some("@Bob"), 4_000).unwrap();
        assert_eq!(m.to_nick.as_deref(), Some("bob"), "by nick, any case");
        assert!(rooms.post("s-cid", None, "x", Some("nobody"), 5_000).is_err());
    }

    #[test]
    fn the_users_words_reach_every_agent_as_the_users() {
        let (mut rooms, id) = room();
        let (_, _, out) = rooms.post(USER, Some(&id), "keep it simple", None, 2_000).unwrap();
        assert_eq!(out.len(), 4, "the orchestrator too");
        assert!(out.iter().all(|d| d.as_user && d.text == "[room cache design #1, the user] keep it simple"));
    }

    #[test]
    fn limits_hold_and_say_why() {
        let (mut rooms, _) = room();
        for i in 0..MAX_POSTS {
            rooms.post("s-ana", None, "idea", None, 10_000 + i as u64).unwrap();
        }
        let err = rooms.post("s-ana", None, "one more", None, 20_000).unwrap_err();
        assert!(err.contains("20 posts in 10 minutes"), "{err}");
        assert!(rooms.post("s-ana", None, "later", None, 20_000 + POST_WINDOW_MS + 1).is_ok(), "the window moves on");
        for i in 0..(MAX_MESSAGES - rooms.rooms[0].messages.len()) {
            let who = ["s-lead", "s-bob", "s-cid"][i % 3];
            rooms.post(who, None, "x", None, 30_000_000 + i as u64 * 100_000).unwrap();
        }
        assert!(rooms.post("s-lead", None, "201st", None, 90_000_000).unwrap_err().contains("200 messages"));
        assert!(rooms.post("s-zed", None, "not a member", None, 1).unwrap_err().contains("no open room"));
    }

    #[test]
    fn only_its_owner_closes_it_and_idle_rooms_close() {
        let (mut rooms, id) = room();
        assert!(rooms.close("s-ana", None, "", 2_000).unwrap_err().contains("only the orchestrator"));
        let (closed, topic, members) = rooms.close("s-lead", None, "LRU, with a scan guard", 2_000).unwrap();
        assert_eq!((closed, topic.as_str(), members.len()), (id, "cache design", 4));
        assert!(rooms.post("s-ana", None, "late", None, 3_000).is_err());
        let mut rooms = Rooms::new(None);
        rooms.open(member("lead"), "t", "", vec![member("ana")], 0).unwrap();
        assert!(rooms.close_idle(IDLE_MS - 1, IDLE_MS).is_empty());
        assert_eq!(rooms.close_idle(IDLE_MS, IDLE_MS).len(), 1);
        assert!(rooms.of("s-ana").is_empty());
    }

    #[test]
    fn a_rooms_log_holds_what_was_said() {
        let dir = std::env::temp_dir().join(format!("mt-rooms-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let mut rooms = Rooms::new(Some(&dir));
        let id = rooms.open(member("lead"), "t", "g", vec![member("ana")], 1).unwrap().id.clone();
        rooms.post("s-ana", None, "hello", None, 2).unwrap();
        rooms.close("s-lead", None, "done", 3).unwrap();
        let lines: Vec<Value> = std::fs::read_to_string(dir.join(format!("{id}.jsonl"))).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1]["message"]["text"], "hello");
        assert_eq!(lines[2]["close"]["summary"], "done");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
