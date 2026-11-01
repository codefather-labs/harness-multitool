//! The Telegram channel of Transport (IF-12): long polling and sending over the Bot API.
//! Who may speak, what a message may do and where the token may appear are specified under the
//! feature `telegram`. A message is data: it reaches an agent as a marked notification (INV-12).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use harness_api::{Secret, TELEGRAM_LABEL};
use serde_json::{Value, json};

use crate::{Daemon, WindowEvent};

/// `sendMessage` takes at most this many characters.
const MAX_TEXT: usize = 4096;
/// The longest flood-control wait a send sits through; more than this is an error for the agent.
const MAX_SEND_WAIT_S: u64 = 30;
const ERROR_WAIT: Duration = Duration::from_secs(5);

pub struct Config {
    pub base_url: String,
    pub token: Secret,
    pub allowed_users: Vec<i64>,
    /// Where a reply goes before anyone allowed has written.
    pub chat: Option<i64>,
    pub poll_timeout_s: u64,
}

/// What the channel knows while it runs.
pub(crate) struct Channel {
    config: Config,
    agent: ureq::Agent,
    reply_to: Option<(i64, Option<i64>)>,
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
        Channel { config, agent, reply_to: None, delivered: 0, ignored: 0, last_error: None }
    }

    pub(crate) fn status(&self) -> Value {
        json!({"on": true, "delivered": self.delivered, "ignored": self.ignored, "can_reply": self.reply_target().is_some(), "last_error": self.last_error})
    }

    fn reply_target(&self) -> Option<(i64, Option<i64>)> {
        self.reply_to.or(self.config.chat.map(|chat| (chat, None)))
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
        Answer::Failed(self.clean(&format!("Telegram refused: {}", parsed["description"].as_str().unwrap_or("no reason given"))))
    }
}

fn offset_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("telegram.json")
}

fn stored_offset(data_dir: &Path) -> i64 {
    let text = std::fs::read_to_string(offset_path(data_dir)).unwrap_or_default();
    serde_json::from_str::<Value>(&text).ok().and_then(|v| v["offset"].as_i64()).unwrap_or(0)
}

pub(crate) fn run(daemon: Arc<Daemon>) {
    let Some(requester) = daemon.telegram.lock().expect("telegram").as_ref().map(Channel::request) else { return };
    let (timeout, allowed) = {
        let channel = daemon.telegram.lock().expect("telegram");
        let config = &channel.as_ref().expect("just seen").config;
        (config.poll_timeout_s, config.allowed_users.clone())
    };
    let mut offset = stored_offset(&daemon.data_dir);
    while !daemon.channels_stopped.load(Ordering::Relaxed) {
        // With no window listening nothing is taken, so nothing is confirmed and lost.
        if daemon.listening_window().is_none() {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        let answer = requester.call("getUpdates", &json!({"offset": offset, "timeout": timeout, "allowed_updates": ["message"]}));
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
        let list = updates.as_array().cloned().unwrap_or_default();
        if list.is_empty() && timeout == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
        for update in list {
            let Some(id) = update["update_id"].as_i64() else { continue };
            if !take(&daemon, &update, id, &allowed) {
                break; // The window went away: this one stays unconfirmed.
            }
            offset = id + 1;
            let _ = std::fs::write(offset_path(&daemon.data_dir), json!({"offset": offset}).to_string());
        }
    }
}

/// One update. False if it has to wait for a window.
fn take(daemon: &Daemon, update: &Value, id: i64, allowed: &[i64]) -> bool {
    let message = &update["message"];
    let from_allowed = message["from"]["id"].as_i64().is_some_and(|user| allowed.contains(&user));
    let (Some(text), true) = (message["text"].as_str(), from_allowed) else {
        // A stranger's text is not kept anywhere, not even in the log.
        daemon.with_channel(|c| c.ignored += 1);
        daemon.trigger_log(&format!("telegram-{id}"), TELEGRAM_LABEL, "", "ignored", "");
        return true;
    };
    let Some(window) = daemon.listening_window() else { return false };
    let uuid = format!("telegram-{id}");
    daemon.deliver(&window, WindowEvent::Notification { uuid: uuid.clone(), label: TELEGRAM_LABEL.to_string(), text: text.to_string() });
    daemon.trigger_log(&uuid, TELEGRAM_LABEL, &window, "handled", "");
    let target = message["chat"]["id"].as_i64().map(|chat| (chat, message["message_thread_id"].as_i64()));
    daemon.with_channel(|c| {
        c.delivered += 1;
        c.reply_to = target.or(c.reply_to);
    });
    true
}

/// The agent's reply: to the chat of the last allowed message, or the configured one.
pub(crate) fn send(daemon: &Daemon, text: &str) -> Result<usize, String> {
    let (requester, target) = {
        let channel = daemon.telegram.lock().expect("telegram");
        let channel = channel.as_ref().ok_or("the Telegram channel is off")?;
        (channel.request(), channel.reply_target())
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
