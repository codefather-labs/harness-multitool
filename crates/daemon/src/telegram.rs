//! The Telegram channel of Transport (IF-12): long polling and sending over the Bot API.
//! Who may speak, what a message may do and where the token may appear are specified under the
//! feature `telegram`. A message is data: it reaches an agent as a marked notification (INV-12).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use harness_api::{Secret, TELEGRAM_LABEL};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Daemon, OpenSession, WindowEvent};

/// `sendMessage` takes at most this many characters.
const MAX_TEXT: usize = 4096;
/// The longest flood-control wait a send sits through; more than this is an error for the agent.
const MAX_SEND_WAIT_S: u64 = 30;
const ERROR_WAIT: Duration = Duration::from_secs(5);
/// Messages kept for one chat with no session to go to; the oldest makes way, and it is said.
const MAX_HELD: usize = 20;

pub struct Config {
    pub base_url: String,
    pub token: Secret,
    pub allowed_users: Vec<i64>,
    /// Where a reply goes before anyone allowed has written.
    pub chat: Option<i64>,
    pub poll_timeout_s: u64,
}

/// A chat, and a forum topic in it.
type Key = (i64, Option<i64>);

/// What the channel keeps across restarts.
#[derive(Default, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    bot: Option<String>,
    #[serde(default)]
    offset: i64,
    #[serde(default)]
    bindings: Vec<Binding>,
    #[serde(default)]
    held: Vec<Held>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Binding {
    chat: i64,
    #[serde(default)]
    topic: Option<i64>,
    session: String,
}

/// A message from someone allowed, waiting until its chat has a session open to go to.
#[derive(Serialize, Deserialize, Clone)]
struct Held {
    chat: i64,
    #[serde(default)]
    topic: Option<i64>,
    text: String,
}

/// What the channel knows while it runs.
pub(crate) struct Channel {
    config: Config,
    agent: ureq::Agent,
    kept: Kept,
    reply_to: Option<Key>,
    delivered: u64,
    ignored: u64,
    last_error: Option<String>,
}

pub enum Answer {
    Result(Value),
    Wait(u64),
    Failed(String),
}

impl Channel {
    pub(crate) fn new(config: Config) -> Channel {
        let timeout = Duration::from_secs(config.poll_timeout_s + 10);
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(Some(timeout)).build().into();
        Channel { config, agent, kept: Kept::default(), reply_to: None, delivered: 0, ignored: 0, last_error: None }
    }

    pub(crate) fn status(&self) -> Value {
        json!({"on": true, "delivered": self.delivered, "ignored": self.ignored, "can_reply": self.reply_target().is_some(), "bound": self.kept.bindings.len(), "held": self.kept.held.len(), "last_error": self.last_error})
    }

    fn reply_target(&self) -> Option<Key> {
        self.reply_to.or(self.config.chat.map(|chat| (chat, None)))
    }

    fn bound_session(&self, key: Key) -> Option<String> {
        self.kept.bindings.iter().find(|b| (b.chat, b.topic) == key).map(|b| b.session.clone())
    }

    fn key_of(&self, session: &str) -> Option<Key> {
        self.kept.bindings.iter().find(|b| b.session == session).map(|b| (b.chat, b.topic))
    }

    /// One session holds one key, and one key one session: a new bind moves both.
    fn bind(&mut self, key: Key, session: &str) {
        self.kept.bindings.retain(|b| (b.chat, b.topic) != key && b.session != session);
        self.kept.bindings.push(Binding { chat: key.0, topic: key.1, session: session.to_string() });
    }

    fn hold(&mut self, key: Key, text: &str) -> bool {
        let of_key = self.kept.held.iter().filter(|h| (h.chat, h.topic) == key).count();
        let dropped = of_key >= MAX_HELD;
        if dropped && let Some(at) = self.kept.held.iter().position(|h| (h.chat, h.topic) == key) {
            self.kept.held.remove(at);
        }
        self.kept.held.push(Held { chat: key.0, topic: key.1, text: text.to_string() });
        dropped
    }

    fn take_held(&mut self, key: Key) -> Vec<String> {
        let (taken, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut self.kept.held).into_iter().partition(|h| (h.chat, h.topic) == key);
        self.kept.held = kept;
        taken.into_iter().map(|h| h.text).collect()
    }

    fn request(&self) -> Requester {
        Requester { agent: self.agent.clone(), base_url: self.config.base_url.trim_end_matches('/').to_string(), token: self.config.token.clone() }
    }
}

#[derive(Clone)]
struct Requester {
    agent: ureq::Agent,
    base_url: String,
    token: Secret,
}

impl Requester {
    /// INV-6: an error may quote the URL, and the URL holds the token.
    fn clean(&self, text: &str) -> String {
        text.replace(self.token.expose(), "<token>")
    }

    fn call(&self, method: &str, body: &Value) -> Answer {
        let url = format!("{}/bot{}/{method}", self.base_url, self.token.expose());
        let mut response = match self.agent.post(&url).send(body.to_string()) {
            Ok(response) => response,
            Err(e) => return Answer::Failed(self.clean(&format!("reaching Telegram: {e}"))),
        };
        let parsed: Value = match response.body_mut().read_json() {
            Ok(parsed) => parsed,
            Err(e) => return Answer::Failed(self.clean(&format!("Telegram's answer: {e}"))),
        };
        if parsed["ok"] == true {
            return Answer::Result(parsed["result"].clone());
        }
        if let Some(wait) = parsed["parameters"]["retry_after"].as_u64() {
            return Answer::Wait(wait);
        }
        if parsed["error_code"] == 409 {
            return Answer::Failed("another program is polling this bot's token: stop it, since two cannot share one".to_string());
        }
        Answer::Failed(self.clean(&format!("Telegram refused: {}", parsed["description"].as_str().unwrap_or("no reason given"))))
    }
}

fn kept_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("telegram.json")
}

fn load_kept(data_dir: &Path) -> Kept {
    let text = std::fs::read_to_string(kept_path(data_dir)).unwrap_or_default();
    serde_json::from_str(&text).unwrap_or_default()
}

/// Whole or not at all, readable by this user only: it holds what people wrote.
fn save_kept(daemon: &Daemon) {
    let text = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| serde_json::to_string(&c.kept).ok());
    let Some(text) = text else { return };
    let path = kept_path(&daemon.data_dir);
    let partial = path.with_extension("partial");
    let mut written = std::fs::write(&partial, text);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        written = written.and_then(|()| std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o600)));
    }
    if written.is_ok() {
        let _ = std::fs::rename(&partial, &path);
    }
}

pub(crate) fn run(daemon: Arc<Daemon>, generation: u64) {
    let current = || daemon.telegram_generation.load(Ordering::SeqCst) == generation;
    let Some(requester) = daemon.telegram.lock().expect("telegram").as_ref().map(Channel::request) else { return };
    let mut kept = load_kept(&daemon.data_dir);
    // Update numbers are a bot's own: another bot starts from its beginning. The id is public.
    let bot = requester.token.expose().split(':').next().unwrap_or_default().to_string();
    if kept.bot.as_deref().is_some_and(|known| known != bot) {
        kept.offset = 0;
    }
    kept.bot = Some(bot);
    let mut offset = kept.offset;
    let (timeout, allowed) = {
        let mut channel = daemon.telegram.lock().expect("telegram");
        let channel = channel.as_mut().expect("just seen");
        channel.kept = kept;
        (channel.config.poll_timeout_s, channel.config.allowed_users.clone())
    };
    let _ = requester.call(
        "setMyCommands",
        &json!({"commands": [
            {"command": "agents", "description": "The open sessions"},
            {"command": "switch", "description": "Send this chat to another session"},
            {"command": "whoami", "description": "The session this chat goes to"},
            {"command": "nick", "description": "Name the session of this chat"},
            {"command": "id", "description": "Your Telegram user id"},
        ]}),
    );
    while current() && !daemon.channels_stopped.load(Ordering::Relaxed) {
        // With nothing to deliver to, nothing is taken, so nothing is confirmed and lost.
        if daemon.listening_window().is_none() && daemon.conversations().is_none_or(|host| host.open_sessions().is_empty()) {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let answer = requester.call("getUpdates", &json!({"offset": offset, "timeout": timeout, "allowed_updates": ["message", "callback_query"]}));
        // Replaced while it waited: what came is the new one's to take.
        if !current() {
            return;
        }
        let updates = match answer {
            Answer::Result(updates) => updates,
            Answer::Wait(seconds) => {
                std::thread::sleep(Duration::from_secs(seconds));
                continue;
            }
            Answer::Failed(message) => {
                daemon.with_channel(|c| c.last_error = Some(message));
                std::thread::sleep(ERROR_WAIT);
                continue;
            }
        };
        daemon.with_channel(|c| c.last_error = None);
        let list = updates.as_array().cloned().unwrap_or_default();
        if list.is_empty() && timeout == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        for update in list {
            let Some(id) = update["update_id"].as_i64() else { continue };
            if !take(&daemon, &requester, &update, id, &allowed) {
                break; // Nothing to deliver to: this one stays unconfirmed.
            }
            offset = id + 1;
            daemon.with_channel(|c| c.kept.offset = offset);
            save_kept(&daemon);
        }
    }
}

/// One update. False if it has to wait for a window.
fn take(daemon: &Daemon, requester: &Requester, update: &Value, id: i64, allowed: &[i64]) -> bool {
    if update["callback_query"].is_object() {
        button(daemon, requester, &update["callback_query"], allowed);
        return true;
    }
    let message = &update["message"];
    // Anyone may ask who they are, to be put on the list: it tells them only their own id, keeps
    // nothing, and reaches no session (CON-87).
    if let (Some(text), Some(user), Some(chat)) = (message["text"].as_str(), message["from"]["id"].as_i64(), message["chat"]["id"].as_i64())
        && text.split_whitespace().next().and_then(|word| word.split('@').next()) == Some("/id")
    {
        say(requester, (chat, message["message_thread_id"].as_i64()), &format!("Your user id: {user}"), None);
        return true;
    }
    let from_allowed = message["from"]["id"].as_i64().is_some_and(|user| allowed.contains(&user));
    let (Some(text), true) = (message["text"].as_str(), from_allowed) else {
        // A stranger's text is not kept anywhere, not even in the log.
        daemon.with_channel(|c| c.ignored += 1);
        daemon.trigger_log(&format!("telegram-{id}"), TELEGRAM_LABEL, "", "ignored", "");
        return true;
    };
    let Some(chat) = message["chat"]["id"].as_i64() else { return true };
    let key: Key = (chat, message["message_thread_id"].as_i64());
    daemon.with_channel(|c| c.reply_to = Some(key));
    if command(daemon, requester, key, text) {
        return true;
    }
    let uuid = format!("telegram-{id}");
    let host = daemon.conversations();
    let open = host.as_ref().map_or_else(Vec::new, |h| h.open_sessions());
    let bound = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| c.bound_session(key));
    // Bound, or the only one there is to bind to.
    let only = (bound.is_none() && open.len() == 1).then(|| open[0].clone());
    if let Some(only) = &only {
        daemon.with_channel(|c| c.bind(key, &only.id));
        save_kept(daemon);
        say(requester, key, &format!("This chat now goes to {}.", only.title), None);
    }
    let target = only.map(|o| o.id).or(bound.clone());
    if let (Some(host), Some(session)) = (&host, &target)
        && host.deliver_to(session, TELEGRAM_LABEL, text)
    {
        delivered(daemon, &uuid, session);
        return true;
    }
    if !open.is_empty() || bound.is_some() {
        hold(daemon, requester, key, text, &open, bound.is_some());
        daemon.trigger_log(&uuid, TELEGRAM_LABEL, "", "held", "");
        return true;
    }
    // No session yet: the window that is there starts one with it.
    let Some(window) = daemon.listening_window() else { return false };
    daemon.deliver(&window, WindowEvent::Notification { uuid: uuid.clone(), label: TELEGRAM_LABEL.to_string(), text: text.to_string() });
    delivered(daemon, &uuid, &window);
    true
}

fn delivered(daemon: &Daemon, uuid: &str, to: &str) {
    daemon.trigger_log(uuid, TELEGRAM_LABEL, to, "handled", "");
    daemon.with_channel(|c| c.delivered += 1);
}

fn hold(daemon: &Daemon, requester: &Requester, key: Key, text: &str, open: &[OpenSession], was_bound: bool) {
    let mut dropped = false;
    daemon.with_channel(|c| dropped = c.hold(key, text));
    save_kept(daemon);
    let why = if was_bound { "The session of this chat is not open." } else { "This chat goes to no session yet." };
    let keep = if dropped { format!(" At most {MAX_HELD} messages wait: the oldest was dropped.") } else { String::new() };
    let what = if open.is_empty() { format!("{why} Your message waits until one is chosen, and none is open now.{keep}") } else { format!("{why} Choose one and your message goes to it.{keep}") };
    say(requester, key, &what, keyboard(open));
}

/// A bot command: never for a session. False if this is not one.
fn command(daemon: &Daemon, requester: &Requester, key: Key, text: &str) -> bool {
    let mut words = text.split_whitespace();
    let Some(first) = words.next().filter(|w| w.starts_with('/')) else { return false };
    let name = first.split('@').next().unwrap_or(first);
    let rest = words.collect::<Vec<_>>().join(" ");
    let open = daemon.conversations().map_or_else(Vec::new, |h| h.open_sessions());
    let bound = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| c.bound_session(key));
    let list = || {
        if open.is_empty() {
            return "No session is open.".to_string();
        }
        let lines: Vec<String> = open.iter().enumerate().map(|(i, s)| format!("{}. {}{}", i + 1, s.title, if bound.as_deref() == Some(s.id.as_str()) { " (current)" } else { "" })).collect();
        format!("Open sessions:\n{}", lines.join("\n"))
    };
    match name {
        "/start" | "/agents" => say(requester, key, &list(), keyboard(&open)),
        "/whoami" => {
            let what = match bound.as_deref().map(|id| open.iter().find(|s| s.id == id).map_or(format!("{id}, not open now"), |s| s.title.clone())) {
                Some(title) => format!("This chat goes to {title}."),
                None => "This chat goes to no session.".to_string(),
            };
            say(requester, key, &what, None);
        }
        "/nick" => {
            let what = match (bound.as_deref(), rest.is_empty()) {
                (None, _) => "This chat goes to no session: /switch to one first.".to_string(),
                (Some(_), true) => "Say the name: /nick name".to_string(),
                (Some(id), false) if daemon.conversations().is_some_and(|host| host.rename(id, &rest)) => format!("This session goes by {rest} now."),
                (Some(_), false) => "The session of this chat is not open.".to_string(),
            };
            say(requester, key, &what, None);
        }
        "/switch" if rest.is_empty() => say(requester, key, &list(), keyboard(&open)),
        "/switch" => match choose(&open, &rest) {
            Ok(session) => switch(daemon, requester, key, &session),
            Err(why) => say(requester, key, &why, keyboard(&open)),
        },
        // Anything else is for the agent, as written.
        _ => return false,
    }
    true
}

/// A session by its number in the list, or by words of its title or id.
fn choose(open: &[OpenSession], asked: &str) -> Result<OpenSession, String> {
    if let Some(found) = asked.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| open.get(i)) {
        return Ok(found.clone());
    }
    let wanted = asked.to_lowercase();
    let named: Vec<&OpenSession> = open.iter().filter(|s| s.nick.as_deref().is_some_and(|nick| nick.to_lowercase() == wanted)).collect();
    if let [one] = named.as_slice() {
        return Ok((*one).clone());
    }
    let found: Vec<&OpenSession> = open.iter().filter(|s| s.title.to_lowercase().contains(&wanted) || s.id.to_lowercase().contains(&wanted)).collect();
    match found.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("No open session matches \"{asked}\".")),
        many => Err(format!("{} sessions match \"{asked}\": choose one.", many.len())),
    }
}

fn switch(daemon: &Daemon, requester: &Requester, key: Key, session: &OpenSession) {
    let mut held = Vec::new();
    daemon.with_channel(|c| {
        c.bind(key, &session.id);
        held = c.take_held(key);
    });
    save_kept(daemon);
    let waited = if held.is_empty() { String::new() } else { format!(" {} waiting message(s) went to it.", held.len()) };
    say(requester, key, &format!("This chat now goes to {}.{waited}", session.title), None);
    if let Some(host) = daemon.conversations() {
        for text in held {
            if host.deliver_to(&session.id, TELEGRAM_LABEL, &text) {
                daemon.with_channel(|c| c.delivered += 1);
            }
        }
    }
}

/// A tap on a button. A stranger's does nothing.
fn button(daemon: &Daemon, requester: &Requester, query: &Value, allowed: &[i64]) {
    if !query["from"]["id"].as_i64().is_some_and(|user| allowed.contains(&user)) {
        daemon.with_channel(|c| c.ignored += 1);
        return;
    }
    let _ = requester.call("answerCallbackQuery", &json!({"callback_query_id": query["id"]}));
    let (Some(data), Some(chat)) = (query["data"].as_str(), query["message"]["chat"]["id"].as_i64()) else { return };
    let key: Key = (chat, query["message"]["message_thread_id"].as_i64());
    let Some(id) = data.strip_prefix("switch:") else { return };
    let open = daemon.conversations().map_or_else(Vec::new, |h| h.open_sessions());
    match open.iter().find(|s| s.id == id) {
        Some(session) => switch(daemon, requester, key, session),
        None => say(requester, key, "That session is not open any more.", keyboard(&open)),
    }
}

/// A button for each session. A session whose id is too long for Telegram's 64 bytes has none.
fn keyboard(open: &[OpenSession]) -> Option<Value> {
    let rows: Vec<Value> = open.iter().filter(|s| s.id.len() + "switch:".len() <= 64).map(|s| json!([{"text": s.title, "callback_data": format!("switch:{}", s.id)}])).collect();
    (!rows.is_empty()).then(|| json!({"inline_keyboard": rows}))
}

/// The channel's own word to a chat. It is news, not a duty: a failure here loses nothing.
fn say(requester: &Requester, key: Key, text: &str, keyboard: Option<Value>) {
    let mut body = json!({"chat_id": key.0, "text": text});
    if let Some(topic) = key.1 {
        body["message_thread_id"] = json!(topic);
    }
    if let Some(keyboard) = keyboard {
        body["reply_markup"] = keyboard;
    }
    let _ = requester.call("sendMessage", &body);
}

/// The agent's reply: to the chat its session is bound to, else that of the last allowed message,
/// else the configured one.
pub(crate) fn send(daemon: &Daemon, text: &str, session: Option<&str>) -> Result<usize, String> {
    let (requester, target) = {
        let channel = daemon.telegram.lock().expect("telegram");
        let channel = channel.as_ref().ok_or("the Telegram channel is off")?;
        (channel.request(), session.and_then(|id| channel.key_of(id)).or(channel.reply_target()))
    };
    let (chat, thread) = target.ok_or("no chat to reply to yet: nobody allowed has written and no chat is configured")?;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Err("nothing to send".to_string());
    }
    let parts: Vec<String> = chars.chunks(MAX_TEXT).map(|part| part.iter().collect()).collect();
    for part in &parts {
        let mut body = json!({"chat_id": chat, "text": part});
        if let Some(thread) = thread {
            body["message_thread_id"] = json!(thread);
        }
        let mut answer = requester.call("sendMessage", &body);
        if let Answer::Wait(seconds) = answer {
            if seconds > MAX_SEND_WAIT_S {
                return Err(format!("Telegram asks to wait {seconds} s before sending"));
            }
            std::thread::sleep(Duration::from_secs(seconds));
            answer = requester.call("sendMessage", &body);
        }
        match answer {
            Answer::Result(_) => {}
            Answer::Wait(seconds) => return Err(format!("Telegram asks to wait {seconds} s before sending")),
            Answer::Failed(message) => return Err(message),
        }
    }
    Ok(parts.len())
}
