//! The contract of a multitool custom callback handler (CON-183, journal#1373).
//!
//! A handler is a Rust program in `.harness/callbacks/<name>/`, built to
//! `target/release/<name>`. The daemon runs it when a callback of one of its types reaches a
//! conversation subscribed to it, and does not wait for it:
//!
//! - **stdin**: the callback's JSON, as it was POSTed (`uuid`, `type`, and the rest);
//! - **arguments**: the `args` of its `config.toml`;
//! - **environment**: `MULTITOOL_CALLBACK_NAME`, `MULTITOOL_CALLBACK_TYPE`,
//!   `MULTITOOL_CALLBACK_SESSIONS` (the conversation it runs for), `MULTITOOL_CALLBACK_REPLY` (the
//!   URL its results go to) and `MULTITOOL_PROJECT` (the project's folder);
//! - **its folder** is the working directory.
//!
//! What it prints goes to the daemon's trigger log only. Its results go to the reply URL, as many
//! as it likes until its time (`timeout_s`, 30 s by default) is over, each one of:
//! `{"deliver": "message", "text": …, "label"?: …}`, `{"deliver": "journal", "text": …}` or
//! `{"deliver": "none"}`. The daemon answers 200 `delivered` or `queued`, 400 for a bad body, 403
//! for a result meant for another conversation, 404 for no such run and 410 once its time is over.
//!
//! ```no_run
//! fn main() -> std::process::ExitCode {
//!     harness_callback::run(|call| {
//!         call.message(&format!("{} came: {}", call.kind, call.input))?;
//!         Ok(())
//!     })
//! }
//! ```
//!
//! Its own logic may run whatever it likes (a Python script, a shell command); the handler is the
//! Rust program that the daemon runs and that answers it.

use serde_json::{Value, json};
use std::fmt;
use std::io::Read;

/// What went wrong with a call or a reply.
#[derive(Debug)]
pub enum Error {
    /// Not run by the daemon: a variable of the contract is missing.
    NotRun(&'static str),
    /// Stdin was not JSON.
    Input(String),
    /// The reply did not reach the daemon.
    Transport(String),
    /// The daemon refused the reply, with its status and what it said.
    Refused(u16, String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotRun(var) => write!(f, "{var} is not set: a handler is run by the multitool daemon"),
            Error::Input(e) => write!(f, "the callback on stdin is not JSON: {e}"),
            Error::Transport(e) => write!(f, "the reply did not reach the daemon: {e}"),
            Error::Refused(status, said) => write!(f, "the daemon refused the reply ({status}): {said}"),
        }
    }
}

impl std::error::Error for Error {}

/// How the daemon took a result: `delivered` to the conversation, or `queued` for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub result: String,
}

/// One run of a handler: the callback it was run for.
#[derive(Debug, Clone)]
pub struct Call {
    /// The callback's name (its folder).
    pub name: String,
    /// The callback's type.
    pub kind: String,
    /// The conversation it runs for.
    pub session: String,
    /// The project's folder.
    pub project: String,
    /// The `args` of its config.toml.
    pub args: Vec<String>,
    /// The callback's JSON, as it was POSTed.
    pub input: Value,
    reply: String,
}

fn var(name: &'static str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| Error::NotRun(name))
}

impl Call {
    /// The call this process was run for: stdin, arguments and environment.
    pub fn read() -> Result<Call, Error> {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map_err(|e| Error::Input(e.to_string()))?;
        let input = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).map_err(|e| Error::Input(e.to_string()))? };
        Ok(Call {
            name: var("MULTITOOL_CALLBACK_NAME")?,
            kind: var("MULTITOOL_CALLBACK_TYPE")?,
            session: var("MULTITOOL_CALLBACK_SESSIONS")?,
            project: var("MULTITOOL_PROJECT")?,
            args: std::env::args().skip(1).collect(),
            input,
            reply: var("MULTITOOL_CALLBACK_REPLY")?,
        })
    }

    /// The callback's uuid.
    pub fn uuid(&self) -> Option<&str> {
        self.input["uuid"].as_str()
    }

    /// The URL results go to.
    pub fn reply_url(&self) -> &str {
        &self.reply
    }

    /// A message to the conversation, shown as `[callback:<name>]: <text>`.
    pub fn message(&self, text: &str) -> Result<Reply, Error> {
        self.send(&json!({"deliver": "message", "text": text}))
    }

    /// A message under a label of its own: `[callback:<label>]: <text>`.
    pub fn message_labeled(&self, label: &str, text: &str) -> Result<Reply, Error> {
        self.send(&json!({"deliver": "message", "label": label, "text": text}))
    }

    /// An entry in the project's journal, tagged `callback` and the callback's name.
    pub fn journal(&self, text: &str) -> Result<Reply, Error> {
        self.send(&json!({"deliver": "journal", "text": text}))
    }

    /// Nothing to deliver: the run is noted in the trigger log as handled.
    pub fn none(&self) -> Result<Reply, Error> {
        self.send(&json!({"deliver": "none"}))
    }

    /// A result as the contract's JSON, for what the helpers do not cover.
    pub fn send(&self, result: &Value) -> Result<Reply, Error> {
        let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
        let mut response = agent.post(&self.reply).header("content-type", "application/json").send(result.to_string()).map_err(|e| Error::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        let said: Value = response.body_mut().read_json().unwrap_or(Value::Null);
        if status == 200 {
            Ok(Reply { result: said["result"].as_str().unwrap_or("delivered").to_string() })
        } else {
            Err(Error::Refused(status, said["error"].as_str().map(str::to_string).unwrap_or_else(|| said.to_string())))
        }
    }
}

/// A whole `main`: reads the call, runs `handle`, and exits 0, or 1 with the error on stderr (the
/// daemon's trigger log).
pub fn run(handle: impl FnOnce(&Call) -> Result<(), Box<dyn std::error::Error>>) -> std::process::ExitCode {
    let call = match Call::read() {
        Ok(call) => call,
        Err(e) => {
            eprintln!("{e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match handle(&call) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}: {e}", call.name);
            std::process::ExitCode::FAILURE
        }
    }
}
