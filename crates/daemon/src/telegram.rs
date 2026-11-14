//! The Telegram channel of Transport (IF-12): long polling and sending over the Bot API.
//! Who may speak, what a message may do and where the token may appear are specified under the
//! feature `telegram`. A message is data: it reaches an agent as a marked notification (INV-12).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use harness_api::{Secret, TELEGRAM_LABEL, MISSING_SPEECH_BACKEND, TELEGRAM_VOICE_LABEL};
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
/// The Bot API gives bots files up to this size.
const MAX_FILE: u64 = 20 * 1024 * 1024;
/// What an agent may send (CON-159, journal#1128): the Bot API's limits.
pub const MAX_PHOTO: u64 = 10 * 1024 * 1024;
pub const MAX_DOCUMENT: u64 = 50 * 1024 * 1024;
pub const MAX_FILES: usize = 10;
const MAX_CAPTION: usize = 1024;
/// How long a file the user sent is kept (CON-160).
const KEEP_RECEIVED: Duration = Duration::from_secs(30 * 24 * 3600);

pub struct Config {
    pub base_url: String,
    pub token: Secret,
    pub allowed_users: Vec<i64>,
    /// Where a reply goes before anyone allowed has written.
    pub chat: Option<i64>,
    pub poll_timeout_s: u64,
    /// What turns a voice note into words; none when speech is off.
    pub transcriber: Option<Transcriber>,
}

/// A voice note's bytes to its words.
pub type Transcriber = Arc<dyn Fn(&[u8]) -> Result<String, String> + Send + Sync>;

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
    /// What polling last met; gone with the next poll that works.
    last_error: Option<String>,
    /// What the channel's own last word met; gone with the next that goes through.
    say_error: Option<String>,
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
        Channel { config, agent, kept: Kept::default(), reply_to: None, delivered: 0, ignored: 0, last_error: None, say_error: None }
    }

    pub(crate) fn status(&self) -> Value {
        json!({"on": true, "delivered": self.delivered, "ignored": self.ignored, "can_reply": self.reply_target().is_some(), "bound": self.kept.bindings.len(), "held": self.kept.held.len(), "last_error": self.last_error.as_ref().or(self.say_error.as_ref())})
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
        // Without this the real Bot API does not read the body at all.
        let response = self.agent.post(&url).header("content-type", "application/json").send(body.to_string());
        self.answer(response)
    }

    /// One file with its fields, as multipart/form-data (CON-159). Built here: the bytes go as they
    /// are, the fields as text, under a boundary the file cannot hold by chance.
    fn upload(&self, method: &str, fields: &[(&str, String)], field: &str, path: &Path) -> Answer {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => return Answer::Failed(format!("reading {}: {e}", path.display())),
        };
        let name = path.file_name().map_or("file".to_string(), |n| n.to_string_lossy().replace(['"', '\r', '\n'], "_"));
        let (boundary, body) = multipart(fields, field, &name, &bytes);
        let url = format!("{}/bot{}/{method}", self.base_url, self.token.expose());
        let response = self.agent.post(&url).header("content-type", &format!("multipart/form-data; boundary={boundary}")).send(&body[..]);
        self.answer(response)
    }

    fn answer(&self, response: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Answer {
        let mut response = match response {
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

impl Requester {
    /// A file a message has, by its id. The address holds the token: errors are cleaned.
    fn fetch_file(&self, id: &str) -> Result<Vec<u8>, String> {
        let path = match self.call("getFile", &json!({"file_id": id})) {
            Answer::Result(file) => file["file_path"].as_str().map(str::to_string).ok_or("Telegram gave no file path")?,
            Answer::Wait(seconds) => return Err(format!("Telegram asks to wait {seconds} s")),
            Answer::Failed(message) => return Err(message),
        };
        let url = format!("{}/file/bot{}/{path}", self.base_url, self.token.expose());
        let mut response = self.agent.get(&url).call().map_err(|e| self.clean(&format!("fetching the file: {e}")))?;
        if !response.status().is_success() {
            return Err(format!("fetching the file: Telegram answered {}", response.status().as_u16()));
        }
        response.body_mut().with_config().limit(MAX_FILE).read_to_vec().map_err(|e| self.clean(&format!("fetching the file: {e}")))
    }
}

/// A multipart/form-data body: the text fields, then one file. Answers the boundary and the body.
pub fn multipart(fields: &[(&str, String)], field: &str, name: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let mut boundary = format!("harness-{:016x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64));
    while bytes.windows(boundary.len()).any(|w| w == boundary.as_bytes()) {
        boundary.push('x');
    }
    let mut body = Vec::with_capacity(bytes.len() + 512);
    for (key, value) in fields {
        body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend(format!("\r\n--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// Whether a file goes as a photo: a picture Telegram shows as one, within its size (CON-159).
pub fn as_photo(path: &Path, size: u64) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp") && size <= MAX_PHOTO
}

/// The files an agent asked to send, checked before anything goes: none missing, no folder, none
/// too big, not too many (CON-159).
pub fn check_files(files: &[PathBuf]) -> Result<Vec<(PathBuf, u64)>, String> {
    if files.len() > MAX_FILES {
        return Err(format!("at most {MAX_FILES} files at once, not {}", files.len()));
    }
    files
        .iter()
        .map(|path| {
            let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if !meta.is_file() {
                return Err(format!("{} is not a file", path.display()));
            }
            if meta.len() > MAX_DOCUMENT {
                return Err(format!("{} is {} MB: Telegram takes at most {} MB", path.display(), meta.len() / (1024 * 1024), MAX_DOCUMENT / (1024 * 1024)));
            }
            Ok((path.clone(), meta.len()))
        })
        .collect()
}

/// Where the files the user sends are kept (CON-160).
fn received_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("telegram").join("files")
}

/// Files the user sent longer ago than KEEP_RECEIVED go.
fn sweep_received(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(received_dir(data_dir)) else { return };
    for entry in entries.flatten() {
        let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > KEEP_RECEIVED);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A file the user sent, fetched and kept: the line the agent reads for it.
fn receive(daemon: &Daemon, requester: &Requester, id: i64, file_id: &str, name: &str, kind: &str) -> String {
    let fetched = requester.fetch_file(file_id).and_then(|bytes| {
        let dir = received_dir(&daemon.data_dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let safe: String = name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' }).collect();
        let path = dir.join(format!("{id}-{safe}"));
        std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok((path, bytes.len()))
    });
    match fetched {
        Ok((path, size)) => format!("attached: {} ({kind}, {} KB)", path.display(), size.div_ceil(1024)),
        Err(reason) => format!("attached: {name} could not be fetched: {reason}"),
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
    sweep_received(&daemon.data_dir);
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
fn take(daemon: &Arc<Daemon>, requester: &Requester, update: &Value, id: i64, allowed: &[i64]) -> bool {
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
        say(daemon, requester, (chat, message["message_thread_id"].as_i64()), &format!("Your user id: {user}"), None);
        return true;
    }
    let from_allowed = message["from"]["id"].as_i64().is_some_and(|user| allowed.contains(&user));
    if !from_allowed {
        // A stranger's message is not kept anywhere, not even in the log, nor a voice note fetched.
        daemon.with_channel(|c| c.ignored += 1);
        daemon.trigger_log(&format!("telegram-{id}"), TELEGRAM_LABEL, "", "ignored", "");
        return true;
    }
    let Some(chat) = message["chat"]["id"].as_i64() else { return true };
    let key: Key = (chat, message["message_thread_id"].as_i64());
    daemon.with_channel(|c| c.reply_to = Some(key));
    // A picture or a file, with its caption: fetched and kept, the agent told where (CON-160).
    let photo = message["photo"].as_array().and_then(|sizes| sizes.last()).and_then(|p| p["file_id"].as_str());
    let document = message["document"]["file_id"].as_str();
    if photo.is_some() || document.is_some() {
        let mut lines = vec![message["caption"].as_str().unwrap_or_default().to_string()];
        if let Some(file) = photo {
            lines.push(receive(daemon, requester, id, file, "photo.jpg", "photo"));
        }
        if let Some(file) = document {
            let name = message["document"]["file_name"].as_str().unwrap_or("file");
            let kind = message["document"]["mime_type"].as_str().unwrap_or("document");
            if message["document"]["file_size"].as_u64().is_some_and(|size| size > MAX_FILE) {
                say(daemon, requester, key, &format!("{name} is over {} MB, which a bot cannot fetch: send it another way.", MAX_FILE / (1024 * 1024)), None);
                lines.push(format!("attached: {name} could not be fetched: over {} MB", MAX_FILE / (1024 * 1024)));
            } else {
                lines.push(receive(daemon, requester, id, file, name, kind));
            }
        }
        let text = lines.join("\n").trim().to_string();
        return route(daemon, requester, key, &format!("telegram-{id}"), TELEGRAM_LABEL, &text);
    }
    let Some(text) = message["text"].as_str() else {
        match message["voice"]["file_id"].as_str() {
            Some(file) => voice(daemon, requester, key, id, file),
            None => say(daemon, requester, key, "Only text, voice, pictures and files reach an agent here.", None),
        }
        return true;
    };
    if command(daemon, requester, key, text) {
        return true;
    }
    route(daemon, requester, key, &format!("telegram-{id}"), TELEGRAM_LABEL, text)
}

/// A voice note, off this thread: other chats wait for nobody's transcribing.
fn voice(daemon: &Arc<Daemon>, requester: &Requester, key: Key, id: i64, file: &str) {
    let transcriber = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| c.config.transcriber.clone());
    let Some(transcriber) = transcriber else {
        no_speech_backend(daemon, requester, key, id);
        return;
    };
    let daemon = daemon.clone();
    let (requester, file) = (requester.clone(), file.to_string());
    std::thread::spawn(move || {
        let uuid = format!("telegram-{id}");
        let words = requester.fetch_file(&file).and_then(|note| transcriber(&note));
        match words {
            Ok(words) if words.trim().is_empty() => say(&daemon, &requester, key, "I heard no words in that voice note.", None),
            Ok(words) => {
                if !route(&daemon, &requester, key, &uuid, TELEGRAM_VOICE_LABEL, &words) {
                    hold(&daemon, &requester, key, &words, &[], false);
                }
            }
            // Set up when the channel started, and gone since.
            Err(why) if why == MISSING_SPEECH_BACKEND => no_speech_backend(&daemon, &requester, key, id),
            Err(why) => {
                say(&daemon, &requester, key, &format!("I could not transcribe that voice note: {why}"), None);
                daemon.trigger_log(&uuid, TELEGRAM_VOICE_LABEL, "", "failed", &why);
            }
        }
    });
}

/// No engine to hear a note with. The user is told, in the chat and in the conversation it goes
/// to; the agent is not, since setting one up is the user's own choice (CON-89).
fn no_speech_backend(daemon: &Daemon, requester: &Requester, key: Key, id: i64) {
    say(daemon, requester, key, MISSING_SPEECH_BACKEND, None);
    if let Some(host) = daemon.conversations() {
        let open = host.open_sessions();
        let bound = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| c.bound_session(key));
        let only = (open.len() == 1).then(|| open[0].id.clone());
        if let Some(session) = bound.or(only) {
            host.notify(&session, TELEGRAM_VOICE_LABEL, MISSING_SPEECH_BACKEND);
        }
    }
    daemon.trigger_log(&format!("telegram-{id}"), TELEGRAM_VOICE_LABEL, "", "no_backend", "");
}

/// A message for whichever session this chat goes to. False if it has to wait for a window.
fn route(daemon: &Daemon, requester: &Requester, key: Key, uuid: &str, label: &str, text: &str) -> bool {
    let host = daemon.conversations();
    let open = host.as_ref().map_or_else(Vec::new, |h| h.open_sessions());
    let bound = daemon.telegram.lock().expect("telegram").as_ref().and_then(|c| c.bound_session(key));
    // Bound, or the only one there is to bind to.
    let only = (bound.is_none() && open.len() == 1).then(|| open[0].clone());
    if let Some(only) = &only {
        daemon.with_channel(|c| c.bind(key, &only.id));
        save_kept(daemon);
        say(daemon, requester, key, &format!("This chat now goes to {}.", only.title), None);
    }
    let target = only.map(|o| o.id).or(bound.clone());
    if let (Some(host), Some(session)) = (&host, &target)
        && host.deliver_to(session, label, text)
    {
        delivered(daemon, uuid, session);
        return true;
    }
    if !open.is_empty() || bound.is_some() {
        hold(daemon, requester, key, text, &open, bound.is_some());
        daemon.trigger_log(uuid, label, "", "held", "");
        return true;
    }
    // No session yet: the window that is there starts one with it.
    let Some(window) = daemon.listening_window() else { return false };
    daemon.deliver(&window, WindowEvent::Notification { uuid: uuid.to_string(), label: label.to_string(), text: text.to_string() });
    delivered(daemon, uuid, &window);
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
    say(daemon, requester, key, &what, keyboard(open));
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
        "/start" | "/agents" => say(daemon, requester, key, &list(), keyboard(&open)),
        "/whoami" => {
            let what = match bound.as_deref().map(|id| open.iter().find(|s| s.id == id).map_or(format!("{id}, not open now"), |s| s.title.clone())) {
                Some(title) => format!("This chat goes to {title}."),
                None => "This chat goes to no session.".to_string(),
            };
            say(daemon, requester, key, &what, None);
        }
        "/nick" => {
            let what = match (bound.as_deref(), rest.is_empty()) {
                (None, _) => "This chat goes to no session: /switch to one first.".to_string(),
                (Some(_), true) => "Say the name: /nick name".to_string(),
                (Some(id), false) if daemon.conversations().is_some_and(|host| host.rename(id, &rest)) => format!("This session goes by {rest} now."),
                (Some(_), false) => "The session of this chat is not open.".to_string(),
            };
            say(daemon, requester, key, &what, None);
        }
        "/switch" if rest.is_empty() => say(daemon, requester, key, &list(), keyboard(&open)),
        "/switch" => match choose(&open, &rest) {
            Ok(session) => switch(daemon, requester, key, &session),
            Err(why) => say(daemon, requester, key, &why, keyboard(&open)),
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
    say(daemon, requester, key, &format!("This chat now goes to {}.{waited}", session.title), None);
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
    // A plan's buttons (CON-161): 'plan:y:<session>' approves, 'plan:n:<session>' does not.
    if let Some(rest) = data.strip_prefix("plan:") {
        let (yes, session) = match rest.split_once(':') {
            Some(("y", session)) => (true, session),
            Some(("n", session)) => (false, session),
            _ => return,
        };
        let outcome = daemon.conversations().ok_or("no conversations are hosted here".to_string()).and_then(|host| host.approve_plan(session, yes));
        if outcome.is_ok() {
            // The buttons go: the answer is given.
            let _ = requester.call("editMessageReplyMarkup", &json!({"chat_id": chat, "message_id": query["message"]["message_id"], "reply_markup": {"inline_keyboard": []}}));
        }
        let said = match outcome {
            Ok(text) => text,
            Err(why) => format!("Nothing changed: {why}"),
        };
        say(daemon, requester, key, &said, None);
        return;
    }
    let Some(id) = data.strip_prefix("switch:") else { return };
    let open = daemon.conversations().map_or_else(Vec::new, |h| h.open_sessions());
    match open.iter().find(|s| s.id == id) {
        Some(session) => switch(daemon, requester, key, session),
        None => say(daemon, requester, key, "That session is not open any more.", keyboard(&open)),
    }
}

/// A button for each session. A session whose id is too long for Telegram's 64 bytes has none.
fn keyboard(open: &[OpenSession]) -> Option<Value> {
    let rows: Vec<Value> = open.iter().filter(|s| s.id.len() + "switch:".len() <= 64).map(|s| json!([{"text": s.title, "callback_data": format!("switch:{}", s.id)}])).collect();
    (!rows.is_empty()).then(|| json!({"inline_keyboard": rows}))
}

/// The channel's own word to a chat. A failure loses no message of the user's; it shows in the status.
fn say(daemon: &Daemon, requester: &Requester, key: Key, text: &str, keyboard: Option<Value>) {
    let mut body = json!({"chat_id": key.0, "text": text});
    if let Some(topic) = key.1 {
        body["message_thread_id"] = json!(topic);
    }
    if let Some(keyboard) = keyboard {
        body["reply_markup"] = keyboard;
    }
    let answer = requester.call("sendMessage", &body);
    daemon.with_channel(|c| c.say_error = if let Answer::Failed(message) = answer { Some(message) } else { None });
}

/// A delegate's plan for the user to approve with a press (CON-161): into its session's chat.
/// `title` is the caller's: this runs while the host may hold its windows, so it asks it nothing.
pub(crate) fn ask_plan(daemon: &Daemon, session: &str, title: &str, plan: &str) -> Result<(), String> {
    let (requester, target) = {
        let channel = daemon.telegram.lock().expect("telegram");
        let channel = channel.as_ref().ok_or("the Telegram channel is off")?;
        (channel.request(), channel.key_of(session).or(channel.reply_target()))
    };
    let key = target.ok_or("no chat to send the plan to yet")?;
    let plan: String = plan.chars().take(3500).collect();
    let text = format!("📋 План агента {title} ждёт одобрения:\n\n{plan}");
    let (yes, no) = (format!("plan:y:{session}"), format!("plan:n:{session}"));
    if yes.len() > 64 {
        say(daemon, &requester, key, &format!("{text}\n\n(Кнопок нет: у этой сессии слишком длинный id. Ответь словами.)"), None);
        return Ok(());
    }
    let keyboard = json!({"inline_keyboard": [[{"text": "✅ Одобрить", "callback_data": yes}, {"text": "✏️ Изменить", "callback_data": no}]]});
    let mut body = json!({"chat_id": key.0, "text": text, "reply_markup": keyboard});
    if let Some(topic) = key.1 {
        body["message_thread_id"] = json!(topic);
    }
    waited(|| requester.call("sendMessage", &body)).map(|_| ())
}

/// The agent's reply: to the chat its session is bound to, else that of the last allowed message,
/// else the configured one.
pub(crate) fn send(daemon: &Daemon, text: &str, files: &[PathBuf], session: Option<&str>) -> Result<usize, String> {
    let (requester, target) = {
        let channel = daemon.telegram.lock().expect("telegram");
        let channel = channel.as_ref().ok_or("the Telegram channel is off")?;
        (channel.request(), session.and_then(|id| channel.key_of(id)).or(channel.reply_target()))
    };
    let (chat, thread) = target.ok_or("no chat to reply to yet: nobody allowed has written and no chat is configured")?;
    // Every file is checked before anything goes (CON-159).
    let files = check_files(files)?;
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() && files.is_empty() {
        return Err("nothing to send".to_string());
    }
    // A text that fits rides on the first file as its caption; otherwise it goes first, alone.
    let caption = (!files.is_empty() && !chars.is_empty() && chars.len() <= MAX_CAPTION).then(|| text.to_string());
    let mut sent = 0;
    if caption.is_none() {
        for part in chars.chunks(MAX_TEXT) {
            let mut body = json!({"chat_id": chat, "text": part.iter().collect::<String>()});
            if let Some(thread) = thread {
                body["message_thread_id"] = json!(thread);
            }
            waited(|| requester.call("sendMessage", &body))?;
            sent += 1;
        }
    }
    for (i, (path, size)) in files.iter().enumerate() {
        let (method, field) = if as_photo(path, *size) { ("sendPhoto", "photo") } else { ("sendDocument", "document") };
        let mut fields = vec![("chat_id", chat.to_string())];
        if let Some(thread) = thread {
            fields.push(("message_thread_id", thread.to_string()));
        }
        if i == 0
            && let Some(caption) = &caption
        {
            fields.push(("caption", caption.clone()));
        }
        waited(|| requester.upload(method, &fields, field, path))?;
        sent += 1;
    }
    Ok(sent)
}

/// One call, waiting once if Telegram asks to and the wait is short.
fn waited(mut call: impl FnMut() -> Answer) -> Result<Value, String> {
    let mut answer = call();
    if let Answer::Wait(seconds) = answer {
        if seconds > MAX_SEND_WAIT_S {
            return Err(format!("Telegram asks to wait {seconds} s before sending"));
        }
        std::thread::sleep(Duration::from_secs(seconds));
        answer = call();
    }
    match answer {
        Answer::Result(value) => Ok(value),
        Answer::Wait(seconds) => Err(format!("Telegram asks to wait {seconds} s before sending")),
        Answer::Failed(message) => Err(message),
    }
}
