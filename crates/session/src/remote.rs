//! The daemon-centric conversation (ADR 0024): sessions run in the daemon, one consistent log
//! there, and a frontend is a client of it. `DaemonHost` is the daemon's side, `RemoteSessions` the
//! client's, `SessionLink` what a frontend holds: the same operations either way.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use harness_api::*;
use harness_core::RunEnd;
use harness_daemon::ConversationHost;
use harness_daemon::client::Client;
use serde_json::{Value, json};

use crate::{Factory, FromSession, Host, Sessions, Settings, TaskRefused};

/// A window that has not polled for this long is gone; its runs finish with nobody watching.
const WINDOW_GONE: Duration = Duration::from_secs(30);
const PUMP: Duration = Duration::from_millis(10);
/// How long a closing daemon waits for its sessions' threads. Idle ones end at once.
const CLOSING: Duration = Duration::from_secs(5);
/// A window polls every couple of seconds. One silent for this long is not there, as far as
/// opening its conversation elsewhere goes.
const WINDOW_SILENT: Duration = Duration::from_secs(5);
/// How long a window waits for one that may have just gone, before it says the other is open.
const WAIT_FOR_OTHER: Duration = Duration::from_secs(8);

fn end_to_json(end: &RunEnd) -> Value {
    match end {
        RunEnd::AwaitingInput => json!("awaiting"),
        RunEnd::Done => json!("done"),
        RunEnd::Abandoned => json!("abandoned"),
        RunEnd::Failed(message) => json!({"failed": message}),
    }
}

fn end_from_json(value: &Value) -> RunEnd {
    match value.as_str() {
        Some("done") => RunEnd::Done,
        Some("abandoned") => RunEnd::Abandoned,
        Some(_) => RunEnd::AwaitingInput,
        None => RunEnd::Failed(value["failed"].as_str().unwrap_or("failed").to_string()),
    }
}

fn to_wire(message: &FromSession) -> Value {
    match message {
        FromSession::Spawn { tab, nick, provider, model, task } => json!({"kind": "spawn", "tab": tab, "nick": nick, "provider": provider, "model": model, "task": task}),
        FromSession::CloseTab { tab } => json!({"kind": "close_tab", "tab": tab}),
        FromSession::Queued { tab, message } => json!({"kind": "queued", "tab": tab, "message": message}),
        FromSession::Entry { tab, entry } => json!({"kind": "entry", "tab": tab, "line": harness_api::event::to_line(entry)}),
        FromSession::Ask { tab, call, capabilities } => json!({"kind": "ask", "tab": tab, "call": call, "capabilities": capabilities}),
        FromSession::Ended { tab, end } => json!({"kind": "ended", "tab": tab, "end": end_to_json(end)}),
        FromSession::Unavailable { tab, message } => json!({"kind": "unavailable", "tab": tab, "message": message}),
        FromSession::Synced { tab, error } => json!({"kind": "synced", "tab": tab, "error": error}),
        FromSession::Progress { tab, progress } => json!({"kind": "progress", "tab": tab, "progress": progress}),
        FromSession::AskWithdrawn { tab, call_id, approved } => json!({"kind": "ask_withdrawn", "tab": tab, "call_id": call_id, "approved": approved}),
    }
}

fn from_wire(value: &Value) -> Option<FromSession> {
    let tab = value["tab"].as_u64()?;
    match value["kind"].as_str()? {
        "queued" => Some(FromSession::Queued { tab, message: serde_json::from_value(value["message"].clone()).ok()? }),
        "spawn" => Some(FromSession::Spawn { tab, nick: value["nick"].as_str()?.to_string(), provider: value["provider"].as_str()?.to_string(), model: value["model"].as_str()?.to_string(), task: value["task"].as_u64()? }),
        "close_tab" => Some(FromSession::CloseTab { tab }),
        "entry" => Some(FromSession::Entry { tab, entry: harness_api::event::from_line(value["line"].as_str()?).ok()? }),
        "ask" => Some(FromSession::Ask { tab, call: serde_json::from_value(value["call"].clone()).ok()?, capabilities: serde_json::from_value(value["capabilities"].clone()).ok()? }),
        "ended" => Some(FromSession::Ended { tab, end: end_from_json(&value["end"]) }),
        "unavailable" => Some(FromSession::Unavailable { tab, message: value["message"].as_str()?.to_string() }),
        "synced" => Some(FromSession::Synced { tab, error: value["error"].as_str().map(str::to_string) }),
        "progress" => Some(FromSession::Progress { tab, progress: serde_json::from_value(value["progress"].clone()).ok()? }),
        "ask_withdrawn" => Some(FromSession::AskWithdrawn { tab, call_id: value["call_id"].as_str()?.to_string(), approved: value["approved"].as_bool().unwrap_or(true) }),
        _ => None,
    }
}

fn tab_of(message: &FromSession) -> u64 {
    match message {
        FromSession::Queued { tab, .. } => *tab,
        FromSession::Spawn { tab, .. } | FromSession::CloseTab { tab } => *tab,
        FromSession::Entry { tab, .. } | FromSession::Ask { tab, .. } | FromSession::Ended { tab, .. } | FromSession::Unavailable { tab, .. } | FromSession::Synced { tab, .. } | FromSession::Progress { tab, .. } | FromSession::AskWithdrawn { tab, .. } => *tab,
    }
}

fn answer_to_json(answer: Answer) -> &'static str {
    match answer {
        Answer::AllowOnce => "once",
        Answer::AlwaysAllow => "always",
        Answer::Deny => "deny",
    }
}

fn answer_from_json(value: &Value) -> Answer {
    match value.as_str() {
        Some("once") => Answer::AllowOnce,
        Some("always") => Answer::AlwaysAllow,
        _ => Answer::Deny,
    }
}

/// Builds what a session gets from this binary, in the daemon as in a frontend.
pub type MakeHost = Arc<dyn Fn(&Settings, Arc<dyn Fn(u64) -> Option<String> + Send + Sync>, Arc<AtomicBool>) -> Host + Send + Sync>;

struct Window {
    sessions: Sessions,
    root: String,
    events: VecDeque<(u64, Value)>,
    next: u64,
    reload: Arc<AtomicBool>,
    notes: Arc<Mutex<HashMap<u64, String>>>,
    /// What each tab runs on, once it is settled.
    choices: crate::Choices,
    /// What a conversation with no choice of its own starts on.
    default: crate::Choice,
    /// Tabs of slaves waiting on their master for a permission.
    slave_asks: std::collections::HashSet<u64>,
    /// What slaves' programs have asked and their masters not yet heard back on (CON-108).
    program_asks: Vec<ProgramAsk>,
    /// What a master allowed for good: tab, tool and target.
    program_allowed: std::collections::HashSet<(u64, String)>,
    /// What it opened with: the rows an orchestra can choose from.
    settings: Settings,
    polled: Instant,
    closed: bool,
    /// Tabs whose prompt nobody has answered yet, with the prompt.
    asking: HashMap<u64, Value>,
    /// Delegates' plans waiting for approval, by tab (CON-163): answered here or on Telegram.
    plan_asks: HashMap<u64, PlanAsk>,
    /// Runs of this window that another window has taken over: tab -> (window, its tab).
    handed: HashMap<u64, (String, u64)>,
    /// This window's tabs showing a run that another window started.
    following: HashMap<u64, Follow>,
    /// What each tab's run has said so far: the log has it only as of its last flush, and a window
    /// taking the run over needs all of it.
    said: HashMap<u64, Vec<Value>>,
    stopped_starting: bool,
    /// The window's environment, for reading a provider's conversations (the daemon's own is not it).
    environment: Option<crate::WindowEnv>,
}

/// A tab showing a run of another window. When it ends the tab has the session itself.
struct Follow {
    window: String,
    tab: u64,
    session: String,
    /// What was asked of the tab meanwhile, in order.
    pending: Vec<Value>,
}

impl Window {
    fn silent(&self) -> bool {
        self.closed || self.polled.elapsed() > WINDOW_SILENT
    }

    fn say(&mut self, event: Value) {
        self.next += 1;
        self.events.push_back((self.next, event));
    }

    /// One operation on this window's own sessions.
    fn apply(&mut self, name: &str, op: &Value) -> Result<Value, String> {
        let tab = op["tab"].as_u64().unwrap_or(0);
        let text = || op["text"].as_str().unwrap_or_default().to_string();
        let on = op["on"].as_bool().unwrap_or(false);
        match name {
            "prompt" => self.sessions.prompt(tab, text()),
            "task" => {
                let steps = serde_json::from_value(op["steps"].clone()).map_err(|e| e.to_string())?;
                return Ok(match self.sessions.task(tab, text(), steps) {
                    Ok(()) => json!({"ok": true}),
                    Err(TaskRefused::Invalid(reason)) => json!({"invalid": reason}),
                    Err(TaskRefused::NotApproved { commands, text }) => json!({"not_approved": {"commands": commands, "text": text}}),
                });
            }
            "approve" => {
                self.sessions.approve_acceptance(&text())?;
            }
            "nick" => self.sessions.nick(tab, text()),
            "step_debug" => self.sessions.step_debug(tab, on),
            "plan" => self.sessions.plan(tab, on),
            "inbound" => self.sessions.inbound(tab, op["label"].as_str().unwrap_or_default().to_string(), text()),
            "answer" => {
                self.asking.remove(&tab);
                self.sessions.answer(tab, answer_from_json(&op["answer"]));
            }
            "stop" => self.sessions.stop(tab),
            "resume" => self.sessions.resume(tab, &text()),
            "rebind" => return Ok(json!({"ok": self.sessions.rebind(tab)})),
            "unqueue" => return Ok(json!({"ok": self.sessions.unqueue(tab, &text())})),
            "session_of" => return Ok(json!({"session": self.sessions.session_of(tab)})),
            "note" => {
                self.notes.lock().expect("notes").insert(tab, text());
            }
            "close" => self.closed = true,
            other => return Err(format!("no such operation: {other}")),
        }
        Ok(json!({"ok": true}))
    }
}

/// The daemon's side: every conversation of every window runs here, and its log is written here.
/// One permission request of a slave's program.
/// A delegate's plan shown in the window as a card (CON-163).
#[derive(Clone)]
struct PlanAsk {
    session: String,
    call_id: String,
}

/// Whether a prompt said to a window is a plan's (the native loop's plan_exit, CON-163).
fn is_plan_ask(asked: &Value) -> bool {
    asked["call"]["name"].as_str().is_some_and(|name| name.ends_with("plan_exit"))
}

struct ProgramAsk {
    id: u64,
    tab: u64,
    /// The tool and its target, for an answer of always.
    key: String,
    answer: Option<String>,
}

/// What the harness itself answers of a slave's program, whose mode lags its own: its own tools; the
/// basics (CON-103); while planning, reading, and writing only docs and debug notes (CON-108).
fn program_may(tool: &str, input: &Value, root: &str, planning: bool) -> bool {
    use harness_api::Capability;
    if tool.starts_with("mcp__harness__") || tool.starts_with("mcp__mode__") {
        return true;
    }
    let text = |field: &str| input[field].as_str().unwrap_or_default().to_string();
    let capability = match tool {
        "Read" => Capability::ReadPath(text("file_path")),
        "Glob" | "Grep" => Capability::ReadPath(input["path"].as_str().unwrap_or(root).to_string()),
        "Edit" | "Write" | "NotebookEdit" => Capability::WritePath(text("file_path")),
        "Bash" => Capability::Command(text("command")),
        _ => return false,
    };
    if !harness_core::plan::slave_basics(std::slice::from_ref(&capability), root) {
        return false;
    }
    if !planning {
        return true;
    }
    match &capability {
        Capability::Command(command) => harness_core::plan::reading_chain(command, root),
        Capability::WritePath(path) => {
            let inside = path.strip_prefix(root).unwrap_or(path).trim_start_matches(['/', '\\']);
            inside.starts_with("docs/") || inside.starts_with(".harness/")
        }
        _ => true,
    }
}

/// What an answer of always covers: this tool, on this command or file.
fn ask_key(tool: &str, input: &Value) -> String {
    let target = ["command", "file_path", "path", "url"].iter().find_map(|field| input[*field].as_str()).map_or_else(|| input.to_string(), str::to_string);
    format!("{tool} {target}")
}

pub struct DaemonHost {
    windows: Mutex<HashMap<String, Window>>,
    /// Messages a window sent and went away from, by session: for whoever opens it next. Only in
    /// memory: a message that never reached the log is not the conversation yet.
    left: Mutex<HashMap<String, Vec<PendingMessage>>>,
    /// The names sessions go by, by session: a log can be far too large to read them from.
    nicks: Mutex<HashMap<String, String>>,
    /// Who is to be told of a slave's program asking: slave and word.
    program_words: Mutex<Vec<(String, String)>>,
    /// When each live agent's master was last reminded of it, or it was first seen (CON-110).
    agent_checks: Mutex<HashMap<String, Instant>>,
    agent_check_every: Duration,
    next_ask: std::sync::atomic::AtomicU64,
    /// What each conversation runs on, by session (CON-94). Kept, so a reload or a change of the
    /// default moves nothing that is already going.
    conversations: Mutex<HashMap<String, crate::Choice>>,
    /// What each conversation has sent other agents lately, by when.
    a2a_sent: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Who masters whom (CON-100). Kept, since a slave stays one through a reload.
    orchestra: Mutex<Orchestra>,
    wake: Condvar,
    data_dir: PathBuf,
    make_host: MakeHost,
}

fn nicks_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("nicks.json")
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Orchestra {
    /// Masters whose mode is on.
    on: std::collections::HashSet<String>,
    /// Slave to master.
    slaves: HashMap<String, String>,
    /// Conversations of a delegate in plan mode.
    #[serde(default)]
    planning: std::collections::HashSet<String>,
    /// Slave to the journal entry that is its task (CON-109).
    #[serde(default)]
    tasks: HashMap<String, u64>,
    /// Slaves that told their master themselves in this run: no notice of its end.
    #[serde(skip)]
    told: std::collections::HashSet<String>,
}

const MAX_SLAVES: usize = 8;
/// How often a master is reminded of each agent it has (CON-110).
const AGENT_CHECK_SECS: u64 = 600;

fn agent_check_every(env: Option<String>) -> Duration {
    Duration::from_secs(env.and_then(|secs| secs.parse().ok()).unwrap_or(AGENT_CHECK_SECS))
}
const ASKS_YOU: &str = "asks you: ";

fn read_orchestra(data_dir: &Path) -> Orchestra {
    std::fs::read_to_string(data_dir.join("daemon").join("orchestra.json")).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn conversations_path(data_dir: &Path) -> PathBuf {
    data_dir.join("daemon").join("conversations.json")
}

fn read_conversations(data_dir: &Path) -> HashMap<String, crate::Choice> {
    std::fs::read_to_string(conversations_path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn read_nicks(data_dir: &Path) -> HashMap<String, String> {
    std::fs::read_to_string(nicks_path(data_dir)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

impl DaemonHost {
    /// A tab has this session: from now on it runs on what the conversation runs on, what it was
    /// told to, or else what is the default now.
    fn settle(&self, window: &Window, tab: u64, session: &str) {
        let mut stored = self.conversations.lock().expect("conversations");
        let mut choices = window.choices.lock().expect("choices");
        if let Some(choice) = stored.get(session) {
            choices.insert(tab, choice.clone());
            return;
        }
        let choice = choices.get(&tab).cloned().unwrap_or_else(|| window.default.clone());
        choices.insert(tab, choice.clone());
        stored.insert(session.to_string(), choice);
        self.save_conversations(&stored);
    }

    fn save_conversations(&self, stored: &HashMap<String, crate::Choice>) {
        let path = conversations_path(&self.data_dir);
        let partial = path.with_extension("partial");
        if serde_json::to_string(stored).is_ok_and(|text| std::fs::write(&partial, text).is_ok()) {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    /// The user chooses for this tab's conversation, and it is brought in line. No other moves.
    fn choose(&self, window: &mut Window, tab: u64, choice: crate::Choice) -> Result<Value, String> {
        if window.sessions.running(tab) || window.asking.contains_key(&tab) {
            return Err("a run is on in this conversation: its adapter can be changed when it ends".to_string());
        }
        window.choices.lock().expect("choices").insert(tab, choice.clone());
        if let Some(session) = window.sessions.session_of(tab) {
            let mut stored = self.conversations.lock().expect("conversations");
            stored.insert(session.to_string(), choice);
            self.save_conversations(&stored);
        }
        Ok(json!({"ok": window.sessions.rebind(tab)}))
    }

    fn save_orchestra(&self, orchestra: &Orchestra) {
        let path = self.data_dir.join("daemon").join("orchestra.json");
        let partial = path.with_extension("partial");
        if serde_json::to_string(orchestra).is_ok_and(|text| std::fs::write(&partial, text).is_ok()) {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    /// The master behind this tab, whose mode is on.
    fn master_of_tab(&self, window: &Window, tab: u64) -> Result<String, String> {
        let session = window.sessions.session_of(tab).ok_or("this tab has no conversation yet")?.to_string();
        if self.orchestra.lock().expect("orchestra").on.contains(&session) {
            Ok(session)
        } else {
            Err("this works only in orchestration mode: call mode__orchestrate with on true first".to_string())
        }
    }

    /// One of this master's own slaves, by nick or id.
    fn own_slave(&self, windows: &HashMap<String, Window>, master: &str, name: &str) -> Result<String, String> {
        let slave = self.resolve_agent(windows, name)?;
        if self.orchestra.lock().expect("orchestra").slaves.get(&slave).map(String::as_str) != Some(master) {
            return Err(format!("{name} is not one of your agents"));
        }
        Ok(slave)
    }

    /// The plan and debug modes of a delegate, whose own loop the harness does not run.
    fn delegate_mode_op(&self, window: &mut Window, tab: u64, name: &str, op: &Value) -> Result<Value, String> {
        let session = window.sessions.session_of(tab).ok_or("this tab has no conversation yet")?.to_string();
        let data = self.data_dir.to_string_lossy().into_owned();
        match name {
            "mode__plan_enter" => {
                let when = {
                    let mut orchestra = self.orchestra.lock().expect("orchestra");
                    let slave = orchestra.slaves.contains_key(&session);
                    orchestra.planning.insert(session);
                    self.save_orchestra(&orchestra);
                    if slave { "Plan mode is on." } else { "Plan mode is on, and takes hold in your process from the next message." }
                };
                window.sessions.plan(tab, true);
                Ok(json!({"text": format!("{when} Until the plan is approved, write nothing but the journal, maps and docs.{}", harness_context_default::PLAN_RULES)}))
            }
            "mode__plan_exit" => {
                if !self.orchestra.lock().expect("orchestra").planning.contains(&session) {
                    return Err("plan mode is not on".to_string());
                }
                // What its run has said so far is not in the log until the run ends.
                let mut log = crate::read_log(&data, &window.root, &session);
                let seen = log.last().map_or(0, |entry| entry.seq);
                log.extend(window.said.get(&tab).into_iter().flatten().filter_map(|said| harness_api::event::from_line(said["line"].as_str()?).ok()).filter(|entry| entry.seq > seen));
                // The user's last words: typed in the window or from their own Telegram (CON-158),
                // never a callback's or another agent's (INV-12).
                let last_user = log
                    .iter()
                    .rev()
                    .find_map(|entry| match &entry.event {
                        harness_api::Event::UserMessage { text } => Some(text.clone()),
                        harness_api::Event::Inbound { label, text } if harness_api::event::is_users_own(label) => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let said = op["user_said"].as_str().map(str::trim).unwrap_or_default();
                let approved = said.chars().count() >= 2 && last_user.contains(said);
                let entered = log.iter().rposition(|entry| matches!(&entry.event, harness_api::Event::ToolRequested { call, .. } if call.name.ends_with("plan_enter"))).unwrap_or(0);
                let called = |tool: &str| log[entered..].iter().any(|entry| matches!(&entry.event, harness_api::Event::ToolRequested { call, .. } if call.name.ends_with(tool)));
                let missing: Vec<&str> = ["journal_add", "mindmap_new", "flowchart_new"].into_iter().filter(|tool| !called(tool)).collect();
                if !missing.is_empty() {
                    return Err(format!("The plan is what the journal and the maps hold, so the next session finds it, and this one has none of: {}. Record it with them, then call this again.", missing.join(", ")));
                }
                // Every plan holds its strategy and its logic (CON-117).
                let task = self.orchestra.lock().expect("orchestra").tasks.get(&session).copied();
                let calls = log[entered..].iter().filter_map(|entry| if let harness_api::Event::ToolRequested { call, .. } = &entry.event { Some((call.name.as_str(), &call.input)) } else { None });
                if let Some(lacks) = harness_core::plan::plan_lacks(calls, task) {
                    return Err(lacks);
                }
                if !approved {
                    // A plan that holds up goes to the user's Telegram as well, with a button to
                    // approve it (CON-161). A slave's plan is its master's, not the user's.
                    let slave = self.orchestra.lock().expect("orchestra").slaves.contains_key(&session);
                    let plan = op["plan"].as_str().unwrap_or_default();
                    let title = self.name_of(window, &session);
                    if !slave && !plan.trim().is_empty() {
                        // A card in the window as well (CON-163), once while one is waiting.
                        if !window.plan_asks.contains_key(&tab) {
                            let id = self.next_ask.fetch_add(1, Ordering::SeqCst);
                            let call_id = format!("plan-ask-{id}");
                            let call = harness_api::ToolCall { id: call_id.clone(), name: "mode__plan_exit".to_string(), input: json!({"plan": plan}) };
                            let capabilities = vec![harness_api::Capability::Command("leave plan mode and start the work".into())];
                            window.say(json!({"kind": "ask", "tab": tab, "call": call, "capabilities": capabilities}));
                            window.plan_asks.insert(tab, PlanAsk { session: session.clone(), call_id });
                        }
                    }
                    let sent = !slave && !plan.trim().is_empty() && harness_daemon::client::Client::find(&self.data_dir).is_some_and(|client| client.telegram_plan(&session, &title, plan).is_ok());
                    return Err(if !slave && !plan.trim().is_empty() {
                        format!("Only the user approves a plan, and here it is by their words or by their answer to the card shown in their window{}. Show them the plan and ask, then end your turn and wait. When they answer there you are told so, and plan mode is off if they approved; if they say to go in words, call this again with their words, exact, in user_said.", if sent { " and on their Telegram" } else { "" })
                    } else {
                        "Only the user approves a plan, and here it is by their words. Show them the plan and ask; when they say to go, call this again with their words, exact, in user_said.".to_string()
                    });
                }
                {
                    let mut orchestra = self.orchestra.lock().expect("orchestra");
                    orchestra.planning.remove(&session);
                    self.save_orchestra(&orchestra);
                }
                window.sessions.plan(tab, false);
                Ok(json!({"text": "The user approved the plan. Plan mode is off. A fix: call debug with on true now, reproduce it before changing anything, verify after. A feature: build it, run it, then debug it the same way."}))
            }
            _ => {
                let on = op["on"] != false;
                if on && self.orchestra.lock().expect("orchestra").planning.contains(&session) {
                    return Err("You are in plan mode: debugging comes after the user approves the plan.".to_string());
                }
                window.sessions.step_debug(tab, on);
                Ok(if on { json!({"text": format!("Debug mode is on.{}", harness_context_default::STEP_DEBUG_RULES)}) } else { json!({"text": "Debug mode is off."}) })
            }
        }
    }

    /// Orchestration mode's operations (CON-100). None if it is not one.
    fn orchestra_op(&self, windows: &mut HashMap<String, Window>, id: &str, tab: u64, name: &str, op: &Value) -> Option<Result<Value, String>> {
        let run = || -> Result<Value, String> {
            let window = windows.get(id).ok_or("this window is not open here")?;
            match name {
                "mode__orchestrate" => {
                    let session = window.sessions.session_of(tab).ok_or("this tab has no conversation yet")?.to_string();
                    let mut orchestra = self.orchestra.lock().expect("orchestra");
                    if orchestra.slaves.contains_key(&session) {
                        return Err("an agent of an orchestra cannot start one of its own".to_string());
                    }
                    let on = op["on"] != false;
                    if on {
                        orchestra.on.insert(session);
                    } else {
                        orchestra.on.remove(&session);
                    }
                    self.save_orchestra(&orchestra);
                    Ok(if on { json!({"role_on": true, "text": format!("Orchestration mode is on.{}", harness_context_default::MASTER_RULES)}) } else { json!({"role_off": true, "text": "Orchestration mode is off. Your agents stay until you close them."}) })
                }
                "mode__agent_models" => {
                    self.master_of_tab(window, tab)?;
                    Ok(json!({"adapters": authorized_adapters(&window.settings, &self.data_dir, window.environment.clone())}))
                }
                "mode__agent_spawn" => {
                    let master = self.master_of_tab(window, tab)?;
                    let nick = op["nick"].as_str().map(str::trim).filter(|nick| !nick.is_empty()).ok_or("nick is required")?;
                    let (adapter, model) = (op["adapter"].as_str().ok_or("adapter is required")?, op["model"].as_str().map(str::trim).filter(|m| !m.is_empty()).ok_or("model is required")?);
                    if self.orchestra.lock().expect("orchestra").slaves.values().filter(|m| **m == master).count() >= MAX_SLAVES {
                        return Err(format!("{MAX_SLAVES} agents is the most a master has: close one first"));
                    }
                    if self.resolve_agent(windows, nick).is_ok() {
                        return Err(format!("{nick} is taken: choose another nick"));
                    }
                    // An agent's contract is an open entry (CON-109).
                    let task = op["task"].as_u64().ok_or("task is required: the id of an open journal entry that is this agent's work. Write it first with journal_add.")?;
                    let memory = crate::project_memory(&self.data_dir.to_string_lossy(), &window.root)?;
                    match pollster::block_on(memory.journal_get(task)).map_err(|e| e.to_string())? {
                        Some(entry) if entry.open => {}
                        Some(_) => return Err(format!("journal#{task} holds no open work: an agent's task is an open entry")),
                        None => return Err(format!("there is no journal#{task}")),
                    }
                    if !authorized_adapters(&window.settings, &self.data_dir, window.environment.clone()).iter().any(|row| row["adapter"] == adapter) {
                        return Err(format!("{adapter} is not authorized here: mode__agent_models lists what is. The user signs in, in the settings; never through an agent."));
                    }
                    Ok(json!({"spawn": {"nick": nick, "provider": adapter, "model": model, "task": task}}))
                }
                "mode__agent_read" => {
                    let master = self.master_of_tab(window, tab)?;
                    let slave = self.own_slave(windows, &master, op["nick"].as_str().ok_or("nick is required")?)?;
                    // Its run so far is not in the log until the run ends.
                    let live = windows.values().find_map(|w| w.sessions.open().into_iter().find(|(_, s)| *s == slave).and_then(|(their_tab, _)| w.said.get(&their_tab).cloned())).unwrap_or_default();
                    Ok(json!({"log": read_agent(&self.data_dir.to_string_lossy(), &window.root, &slave, &live)}))
                }
                "mode__agent_answer" => {
                    let master = self.master_of_tab(window, tab)?;
                    let slave = self.own_slave(windows, &master, op["nick"].as_str().ok_or("nick is required")?)?;
                    if !matches!(op["answer"].as_str(), Some("once" | "always" | "deny")) {
                        return Err("answer is once, always or deny".to_string());
                    }
                    Ok(json!({"answer_slave": slave, "answer": op["answer"]}))
                }
                "mode__agent_close" => {
                    let master = self.master_of_tab(window, tab)?;
                    let slave = self.own_slave(windows, &master, op["nick"].as_str().ok_or("nick is required")?)?;
                    Ok(json!({"close": slave}))
                }
                _ => Err(String::new()),
            }
        };
        if !name.starts_with("mode__") {
            return None;
        }
        let reply = run();
        // Into its log too, whoever runs its turns; the tab and a later prompt read it from there.
        if let (Ok(answer), Some(window)) = (&reply, windows.get_mut(id))
            && (answer.get("role_on").is_some() || answer.get("role_off").is_some())
        {
            window.sessions.role(tab, answer.get("role_on").map(|_| "master".to_string()), None, None);
        }
        Some(match reply {
            Ok(reply) if reply.get("spawn").is_some() => {
                let spawn = &reply["spawn"];
                let window = windows.get_mut(id)?;
                window.say(json!({"kind": "spawn", "tab": tab, "nick": spawn["nick"], "provider": spawn["provider"], "model": spawn["model"], "task": spawn["task"]}));
                Ok(json!({"text": format!("Its tab is opening, and it starts on journal#{} at once. It asks you when it is done: {}", spawn["task"], spawn["nick"])}))
            }
            Ok(reply) if reply.get("answer_slave").is_some() => {
                let slave = reply["answer_slave"].as_str().unwrap_or_default();
                for window in windows.values_mut().filter(|w| !w.closed) {
                    if let Some((their_tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == slave) {
                        let answer = reply["answer"].as_str().unwrap_or("deny").to_string();
                        // What a program that has stopped asked is no one's to answer now.
                        if !window.sessions.running(their_tab) {
                            window.program_asks.retain(|ask| ask.tab != their_tab);
                        }
                        if let Some(ask) = window.program_asks.iter_mut().find(|ask| ask.tab == their_tab && ask.answer.is_none()) {
                            if answer == "always" {
                                window.program_allowed.insert((their_tab, ask.key.clone()));
                            }
                            ask.answer = Some(answer);
                            return Some(Ok(json!({"text": "Answered."})));
                        }
                        if !window.slave_asks.remove(&their_tab) {
                            return Some(Err("it is not waiting on a permission".to_string()));
                        }
                        window.sessions.answer(their_tab, answer_from_json(&reply["answer"]));
                        return Some(Ok(json!({"text": "Answered."})));
                    }
                }
                Err("it is not open".to_string())
            }
            Ok(reply) if reply.get("close").is_some() => {
                let slave = reply["close"].as_str().unwrap_or_default().to_string();
                {
                    let mut orchestra = self.orchestra.lock().expect("orchestra");
                    orchestra.slaves.remove(&slave);
                    orchestra.tasks.remove(&slave);
                    orchestra.planning.remove(&slave);
                    self.save_orchestra(&orchestra);
                }
                for window in windows.values_mut().filter(|w| !w.closed) {
                    if let Some((their_tab, _)) = window.sessions.open().into_iter().find(|(_, s)| *s == slave) {
                        window.sessions.stop(their_tab);
                        window.sessions.forget(their_tab);
                        window.say(json!({"kind": "close_tab", "tab": their_tab}));
                    }
                }
                Ok(json!({"text": "Closed. What it wrote to the journal and the project stays."}))
            }
            other => other,
        })
    }

    /// What a master is to know of its agents reaches it as data. An agent that told it in that run
    /// needs no notice.
    fn tell_masters(&self, windows: &mut HashMap<String, Window>, waiting: Vec<(String, String)>, ended: Vec<(String, String)>) -> bool {
        let data = self.data_dir.to_string_lossy().into_owned();
        let mut words: Vec<(String, String, String)> = Vec::new();
        {
            let mut orchestra = self.orchestra.lock().expect("orchestra");
            let nicks = self.nicks.lock().expect("nicks");
            let name = |slave: &str| nicks.get(slave).cloned().unwrap_or_else(|| slave.to_string());
            for (slave, what) in waiting {
                if let Some(master) = orchestra.slaves.get(&slave).cloned() {
                    words.push((master, name(&slave), what.clone()));
                    // It has told its master what it waits for: its run's end adds nothing.
                    if what.starts_with(ASKS_YOU) {
                        orchestra.told.insert(slave);
                    }
                }
            }
            for (slave, root) in ended {
                let Some(master) = orchestra.slaves.get(&slave).cloned() else { continue };
                if orchestra.told.remove(&slave) {
                    continue;
                }
                words.push((master, name(&slave), run_end_word(&read_agent(&data, &root, &slave, &[]))));
            }
        }
        let any = !words.is_empty();
        for (master, slave, what) in words {
            for window in windows.values_mut().filter(|w| !w.closed) {
                if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| *s == master) {
                    window.sessions.inbound(tab, format!("agent:{slave}"), what.clone());
                }
            }
        }
        any
    }

    /// Each live agent's master, once a period, in case the agent has gone to sleep (CON-110). The
    /// harness keeps this, so it starts with the agent and ends with its session by itself.
    fn remind_masters(&self, windows: &mut HashMap<String, Window>) -> bool {
        let now = Instant::now();
        let mut due: Vec<(String, String)> = Vec::new();
        {
            let mut orchestra = self.orchestra.lock().expect("orchestra");
            let nicks = self.nicks.lock().expect("nicks");
            let mut checks = self.agent_checks.lock().expect("checks");
            checks.retain(|slave, _| orchestra.slaves.contains_key(slave));
            let mut gone = Vec::new();
            for (slave, master) in &orchestra.slaves {
                let last = checks.entry(slave.clone()).or_insert(now);
                if now.duration_since(*last) < self.agent_check_every {
                    continue;
                }
                *last = now;
                // Its window went, and its tab with it: there is nothing to check (journal#595).
                if !windows.values().filter(|w| !w.closed).any(|w| w.sessions.open().into_iter().any(|(_, s)| s == *slave)) {
                    let name = nicks.get(slave).map_or(slave.as_str(), String::as_str);
                    let task = orchestra.tasks.get(slave).map_or_else(|| "no entry".to_string(), |task| format!("journal#{task}"));
                    due.push((master.clone(), format!("{name} is gone: its window closed with its tab, and its work on {task} stopped where it was. What it wrote stays. Start another agent on that entry if the work goes on.")));
                    gone.push(slave.clone());
                    continue;
                }
                let running = windows.values().filter(|w| !w.closed).any(|w| w.sessions.open().into_iter().any(|(tab, s)| s == *slave && w.sessions.running(tab)));
                let name = nicks.get(slave).map_or(slave.as_str(), String::as_str);
                let task = orchestra.tasks.get(slave).map_or_else(|| "no entry".to_string(), |task| format!("journal#{task}"));
                let state = if running { "running" } else { "not running" };
                due.push((master.clone(), format!("Time to check on {name}, on {task}: it is {state}. See what it has done with mode__agent_read, and move it on or close it if it has stopped.")));
            }
            for slave in &gone {
                orchestra.slaves.remove(slave);
                orchestra.tasks.remove(slave);
                orchestra.planning.remove(slave);
            }
            if !gone.is_empty() {
                self.save_orchestra(&orchestra);
            }
        }
        let any = !due.is_empty();
        for (master, what) in due {
            for window in windows.values_mut().filter(|w| !w.closed) {
                if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| *s == master) {
                    window.sessions.inbound(tab, harness_api::HARNESS_LABEL.to_string(), what.clone());
                }
            }
        }
        any
    }

    /// A window has opened the tab of a new agent: it starts here, and is told what it is.
    fn adopt(&self, window: &mut Window, op: &Value) -> Result<Value, String> {
        let tab = op["tab"].as_u64().ok_or("tab is required")?;
        let master_tab = op["master_tab"].as_u64().ok_or("master_tab is required")?;
        let nick = op["nick"].as_str().ok_or("nick is required")?.to_string();
        let choice = crate::Choice { provider: op["provider"].as_str().ok_or("provider is required")?.to_string(), model: op["model"].as_str().ok_or("model is required")?.to_string(), slave: true };
        let master = window.sessions.session_of(master_tab).ok_or("its master is gone")?.to_string();
        let master_name = self.name_of(window, &master);
        window.choices.lock().expect("choices").insert(tab, choice);
        window.sessions.nick(tab, nick.clone());
        let task = op["task"].as_u64().ok_or("task is required")?;
        let session = window.sessions.session_of(tab).ok_or("it did not start")?.to_string();
        self.settle(window, tab, &session);
        self.remember_nick(&session, &nick);
        {
            let mut orchestra = self.orchestra.lock().expect("orchestra");
            orchestra.tasks.insert(session.clone(), task);
            orchestra.slaves.insert(session, master);
            self.save_orchestra(&orchestra);
        }
        window.sessions.role(tab, Some("slave".to_string()), Some(master_name.clone()), Some(task));
        window.sessions.inbound(tab, harness_api::HARNESS_LABEL.to_string(), format!("You have just been started as an agent working for the orchestrator {master_name}. Your task is journal#{task}. Read the agent workflow specification, {}, read the entry with journal_get, and work on it now.", crate::agent_workflow_file(&self.data_dir.to_string_lossy())));
        Ok(json!({"ok": true}))
    }

    /// What a conversation goes by, for other agents.
    /// A delegate's plan decided, by a press on Telegram or the window's card (CON-161, CON-163):
    /// approved, its session leaves plan mode; either way its agent is told, as the user's words.
    fn decide_plan(&self, window: &mut Window, tab: u64, session: &str, yes: bool, from_telegram: bool) {
        if yes {
            {
                let mut orchestra = self.orchestra.lock().expect("orchestra");
                orchestra.planning.remove(session);
                self.save_orchestra(&orchestra);
            }
            window.sessions.plan(tab, false);
        }
        let place = if from_telegram { "в Telegram" } else { "в терминале" };
        let text = if yes { format!("✅ План одобрен {place}. Plan mode is off: go ahead with the plan.") } else { format!("✏️ План не одобрен {place}: спроси, что изменить.") };
        if from_telegram {
            window.sessions.inbound(tab, harness_api::TELEGRAM_LABEL.to_string(), text);
        } else {
            window.sessions.prompt(tab, text);
        }
    }

    /// Telegram's message of a plan answered in the window says so and loses its buttons
    /// (CON-163). Off this thread: the daemon is asked over its socket.
    fn plan_done(&self, session: &str, approved: bool) {
        let (data, session) = (self.data_dir.clone(), session.to_string());
        std::thread::spawn(move || {
            if let Some(client) = harness_daemon::client::Client::find(&data) {
                let _ = client.telegram_plan_done(&session, approved);
            }
        });
    }

    fn name_of(&self, window: &Window, session: &str) -> String {
        self.nicks.lock().expect("nicks").get(session).cloned().unwrap_or_else(|| crate::title_of(&self.data_dir.to_string_lossy(), &window.root, session))
    }

    /// Each open conversation on this machine (CON-99).
    fn agent_list(&self, windows: &HashMap<String, Window>, asking: Option<&str>) -> Value {
        // A slave has one contact (CON-100).
        let only = asking.and_then(|session| self.orchestra.lock().expect("orchestra").slaves.get(session).cloned());
        let stored = self.conversations.lock().expect("conversations");
        let nicks = self.nicks.lock().expect("nicks");
        let mut agents = Vec::new();
        for window in windows.values().filter(|w| !w.closed) {
            for (_, session) in window.sessions.open() {
                if only.as_ref().is_some_and(|only| *only != session) {
                    continue;
                }
                let choice = stored.get(&session);
                agents.push(json!({
                    "nick": nicks.get(&session),
                    "id": session,
                    "adapter": choice.map(|c| c.provider.as_str()),
                    "model": choice.map(|c| c.model.as_str()),
                    "folder": window.root,
                    "running": window.sessions.runs(&session),
                }));
            }
        }
        json!({"agents": agents})
    }

    /// A message to another agent, as data from outside (CON-99, INV-12).
    fn agent_send(&self, windows: &mut HashMap<String, Window>, id: &str, tab: u64, op: &Value) -> Result<Value, String> {
        const MAX_SENDS: usize = 60;
        const SEND_WINDOW: Duration = Duration::from_secs(600);
        let to = op["to"].as_str().map(str::trim).filter(|to| !to.is_empty()).ok_or("to is required: a nick or an id")?;
        let text = op["text"].as_str().map(str::trim).filter(|text| !text.is_empty()).ok_or("text is required")?;
        let (sender, from) = {
            let window = windows.get(id).ok_or("this window is not open here")?;
            let session = window.sessions.session_of(tab).ok_or("this tab has no conversation yet")?.to_string();
            let from = self.name_of(window, &session);
            (session, from)
        };
        let target = self.resolve_agent(windows, to)?;
        if target == sender {
            return Err("that is you: an agent does not message itself".to_string());
        }
        let (sender_is_slave, to_own_slave) = {
            let mut orchestra = self.orchestra.lock().expect("orchestra");
            let master = orchestra.slaves.get(&sender).cloned();
            if let Some(master) = &master {
                if *master != target {
                    return Err(format!("you answer to {} alone: an agent of an orchestra messages only its orchestrator", master));
                }
                orchestra.told.insert(sender.clone());
            }
            (master.is_some(), orchestra.slaves.get(&target) == Some(&sender))
        };
        let _ = sender_is_slave;
        {
            let mut sent = self.a2a_sent.lock().expect("a2a");
            let times = sent.entry(sender).or_default();
            while times.front().is_some_and(|at| at.elapsed() > SEND_WINDOW) {
                times.pop_front();
            }
            if times.len() >= MAX_SENDS {
                return Err(format!("{MAX_SENDS} messages to other agents in 10 minutes is the limit: wait, or say so to the user"));
            }
            times.push_back(Instant::now());
        }
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((their_tab, _)) = window.sessions.open().into_iter().find(|(_, s)| *s == target) {
                let queued = window.sessions.runs(&target);
                if to_own_slave {
                    // The orchestrator stands for the user there.
                    window.sessions.prompt(their_tab, text.to_string());
                } else {
                    window.sessions.inbound(their_tab, format!("agent:{from}"), text.to_string());
                }
                return Ok(json!({"delivered_to": target, "queued": queued}));
            }
        }
        Err(format!("no open conversation {to}"))
    }

    /// A nick, any case, or an id, among what is open.
    fn resolve_agent(&self, windows: &HashMap<String, Window>, to: &str) -> Result<String, String> {
        let nicks = self.nicks.lock().expect("nicks");
        let open: Vec<String> = windows.values().filter(|w| !w.closed).flat_map(|w| w.sessions.open().into_iter().map(|(_, session)| session)).collect();
        if open.iter().any(|session| session == to) {
            return Ok(to.to_string());
        }
        let found: Vec<&String> = open.iter().filter(|session| nicks.get(*session).is_some_and(|nick| nick.eq_ignore_ascii_case(to))).collect();
        match found.as_slice() {
            [] => Err(format!("no open conversation goes by {to}: agent_list shows who is here")),
            [one] => Ok((*one).clone()),
            many => Err(format!("several go by {to}, so send by id: {}", many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))),
        }
    }

    fn remember_nick(&self, session: &str, nick: &str) {
        let mut nicks = self.nicks.lock().expect("nicks");
        if nicks.get(session).map(String::as_str) == Some(nick) {
            return;
        }
        nicks.insert(session.to_string(), nick.to_string());
        let path = nicks_path(&self.data_dir);
        let partial = path.with_extension("partial");
        if serde_json::to_string(&*nicks).is_ok_and(|text| std::fs::write(&partial, text).is_ok()) {
            let _ = std::fs::rename(&partial, &path);
        }
    }

    pub fn new(data_dir: &Path, make_host: MakeHost) -> Arc<DaemonHost> {
        let host = Arc::new(DaemonHost { agent_checks: Mutex::new(HashMap::new()), agent_check_every: agent_check_every(std::env::var("MULTITOOL_AGENT_CHECK_SECS").ok()), program_words: Mutex::new(Vec::new()), next_ask: Default::default(), windows: Mutex::new(HashMap::new()), left: Mutex::new(HashMap::new()), nicks: Mutex::new(read_nicks(data_dir)), conversations: Mutex::new(read_conversations(data_dir)), a2a_sent: Mutex::default(), orchestra: Mutex::new(read_orchestra(data_dir)), wake: Condvar::new(), data_dir: data_dir.to_path_buf(), make_host });
        let pump = host.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(PUMP);
                pump.pump();
            }
        });
        host
    }

    /// Takes what the sessions said and queues it for their windows.
    fn pump(&self) {
        let mut windows = self.windows.lock().expect("windows");
        let mut any = false;
        // A handed-over run speaks to the window that took it.
        let mut handed = Vec::new();
        // What a master is to be told of its agents.
        let mut waiting_word: Vec<(String, String)> = Vec::new();
        let mut ended_runs: Vec<(String, String)> = Vec::new();
        for (id, window) in windows.iter_mut() {
            let messages = window.sessions.drain();
            let reload = window.reload.swap(false, Ordering::SeqCst);
            if window.polled.elapsed() > WINDOW_GONE {
                window.closed = true;
            }
            // Gone: what it was still going to send does not start by itself. Its sender may be
            // on the conversation in another program by now (INV-20).
            if window.silent() && !window.stopped_starting {
                window.stopped_starting = true;
                let mut left = self.left.lock().expect("left");
                for (session, waiting) in window.sessions.stop_starting() {
                    left.entry(session).or_default().extend(waiting);
                }
            }
            let (theirs, own): (Vec<_>, Vec<_>) = messages.into_iter().partition(|m| window.handed.contains_key(&tab_of(m)));
            for message in theirs {
                let (to, tab) = window.handed[&tab_of(&message)].clone();
                handed.push((id.clone(), tab_of(&message), to, tab, message));
            }
            for message in &own {
                match message {
                    FromSession::Ask { tab, call, .. } => {
                        let slave = window.sessions.session_of(*tab).map(str::to_string).filter(|session| self.orchestra.lock().expect("orchestra").slaves.contains_key(session));
                        // A slave's permission is its master's to give (CON-100): no card here.
                        if let Some(slave) = slave {
                            window.slave_asks.insert(*tab);
                            waiting_word.push((slave, format!("asks your permission: {} {}\nAnswer with mode__agent_answer: once, always or deny.", call.name, call.input)));
                        } else {
                            window.asking.insert(*tab, to_wire(message));
                            // The native loop's plan is asked on Telegram too (CON-163).
                            if call.name.ends_with("plan_exit")
                                && let Some(session) = window.sessions.session_of(*tab).map(str::to_string)
                            {
                                let (data, title, plan) = (self.data_dir.clone(), self.name_of(window, &session), call.input["plan"].as_str().unwrap_or_default().to_string());
                                std::thread::spawn(move || {
                                    if let Some(client) = harness_daemon::client::Client::find(&data) {
                                        let _ = client.telegram_plan(&session, &title, &plan);
                                    }
                                });
                            }
                        }
                    }
                    FromSession::Entry { tab, entry } => {
                        if let harness_api::Event::AgentName { name, .. } = &entry.event
                            && let Some(session) = window.sessions.session_of(*tab)
                        {
                            self.remember_nick(session, name);
                        }
                        // A slave's question goes to its master (CON-100).
                        if let harness_api::Event::ToolRequested { call, .. } = &entry.event
                            && call.name == "ask_user"
                            && let Some(session) = window.sessions.session_of(*tab)
                            && let Ok(question) = harness_api::question::Question::parse(&call.input)
                        {
                            let options: String = question.options.iter().enumerate().map(|(i, o)| format!("\n{}. {}", i + 1, o.label)).collect();
                            waiting_word.push((session.to_string(), format!("{ASKS_YOU}{}{options}\nAnswer it with agent_send.", question.text)));
                        }
                        window.said.entry(*tab).or_default().push(to_wire(message));
                    }
                    FromSession::Ended { tab, .. } => {
                        window.said.remove(tab);
                        if let Some(session) = window.sessions.session_of(*tab) {
                            ended_runs.push((session.to_string(), window.root.clone()));
                        }
                    }
                    FromSession::Queued { .. } => {},
                    FromSession::Unavailable { .. } | FromSession::Synced { .. } | FromSession::Progress { .. } | FromSession::Spawn { .. } | FromSession::CloseTab { .. } | FromSession::AskWithdrawn { .. } => {}
                }
            }
            if window.closed {
                window.events.clear();
                continue;
            }
            for message in own {
                if matches!(&message, FromSession::Ask { tab, .. } if window.slave_asks.contains(tab)) {
                    continue;
                }
                window.say(to_wire(&message));
                any = true;
            }
            if reload {
                window.say(json!({"kind": "reload"}));
                any = true;
            }
        }
        for (from, from_tab, to, tab, message) in handed {
            any |= Self::bring(&mut windows, &from, from_tab, &to, tab, message);
        }
        // Nobody is there to answer: a prompt is a denial, and the run goes on to its end.
        let unanswered: Vec<(String, u64)> = windows.iter_mut().filter(|(_, w)| w.closed).flat_map(|(id, w)| w.asking.drain().map(|(tab, _)| (id.clone(), tab)).collect::<Vec<_>>()).collect();
        for (id, tab) in unanswered {
            let source = windows.get(&id).and_then(|w| w.following.get(&tab)).map(|f| (f.window.clone(), f.tab));
            let (owner, owner_tab) = source.unwrap_or((id, tab));
            if let Some(window) = windows.get(&owner) {
                window.sessions.answer(owner_tab, Answer::Deny);
            }
        }
        // A closed window stays until its runs have finished: the log gets their end.
        windows.retain(|_, w| !w.closed || w.sessions.busy() || !w.handed.is_empty() || !w.following.is_empty());
        waiting_word.append(&mut self.program_words.lock().expect("words"));
        any |= self.tell_masters(&mut windows, waiting_word, ended_runs);
        any |= self.remind_masters(&mut windows);
        if any {
            self.wake.notify_all();
        }
    }

    /// One message of a handed-over run reaches the tab following it. When the run has ended, the
    /// session is that tab's own, and what was asked of it meanwhile goes in.
    fn bring(windows: &mut HashMap<String, Window>, from: &str, from_tab: u64, to: &str, tab: u64, message: FromSession) -> bool {
        let ended = matches!(message, FromSession::Ended { .. });
        let retabbed = match message {
            FromSession::Queued { message, .. } => FromSession::Queued { tab, message },
            FromSession::Entry { entry, .. } => FromSession::Entry { tab, entry },
            FromSession::Ask { call, capabilities, .. } => FromSession::Ask { tab, call, capabilities },
            FromSession::Ended { end, .. } => FromSession::Ended { tab, end },
            FromSession::Unavailable { message, .. } => FromSession::Unavailable { tab, message },
            FromSession::Synced { error, .. } => FromSession::Synced { tab, error },
            FromSession::Progress { progress, .. } => FromSession::Progress { tab, progress },
            FromSession::Spawn { nick, provider, model, task, .. } => FromSession::Spawn { tab, nick, provider, model, task },
            FromSession::CloseTab { .. } => FromSession::CloseTab { tab },
            FromSession::AskWithdrawn { call_id, approved, .. } => FromSession::AskWithdrawn { tab, call_id, approved },
        };
        let mut said = false;
        if let (FromSession::Entry { .. }, Some(source)) = (&retabbed, windows.get_mut(from)) {
            source.said.entry(from_tab).or_default().push(to_wire(&retabbed));
        }
        if let Some(window) = windows.get_mut(to) {
            if let FromSession::Ask { .. } = retabbed {
                window.asking.insert(tab, to_wire(&retabbed));
            }
            if !window.closed {
                window.say(to_wire(&retabbed));
                said = true;
            }
        }
        if !ended {
            return said;
        }
        if let Some(source) = windows.get_mut(from) {
            source.handed.remove(&from_tab);
            source.sessions.forget(from_tab);
        }
        if let Some(window) = windows.get_mut(to)
            && let Some(follow) = window.following.remove(&tab)
        {
            window.sessions.resume(tab, &follow.session);
            for op in follow.pending {
                let _ = window.apply(op["op"].as_str().unwrap_or_default(), &op);
            }
        }
        said
    }

    /// Opening a session some other window holds. One writer to a log (CON-77): a window still
    /// there keeps it; from one that went away, the session comes here, with its run if it is on.
    fn take_over(windows: &mut HashMap<String, Window>, id: &str, tab: u64, session: &str) -> Option<Value> {
        let root = windows.get(id)?.root.clone();
        let (other, other_tab) = windows.iter().filter(|(other, w)| other.as_str() != id && w.root == root).find_map(|(other, w)| Some((other.clone(), w.sessions.tab_of(session)?)))?;
        let source = &windows[&other];
        // A process has one window: an older one of this process is what a hot reload left.
        let this_process = |window: &str| window.split('-').nth(1).map(str::to_string);
        let left_behind = this_process(&other).is_some() && this_process(&other) == this_process(id);
        let shown_by = source.handed.get(&other_tab).map(|(window, _)| window.clone());
        let there = match &shown_by {
            Some(window) if window == id => false,
            Some(window) => windows.get(window).is_some_and(|w| !w.silent()),
            None => !source.silent() && !left_behind,
        };
        if there {
            return Some(json!({"elsewhere": true}));
        }
        let running = source.sessions.runs(session);
        if let Some(window) = shown_by.as_ref().and_then(|window| windows.get_mut(window)) {
            window.following.retain(|_, f| f.session != session);
        }
        let source = windows.get_mut(&other)?;
        source.closed = true;
        if !running {
            source.handed.remove(&other_tab);
            source.sessions.forget(other_tab);
            return None;
        }
        let waiting = source.sessions.hand_over(other_tab);
        let said = source.said.get(&other_tab).cloned().unwrap_or_default();
        let prompt = source.asking.remove(&other_tab);
        source.handed.insert(other_tab, (id.to_string(), tab));
        let window = windows.get_mut(id)?;
        window.sessions.forget(tab);
        let pending = waiting.into_iter().map(|message| match message {
            PendingMessage::User { text } => json!({"op": "prompt", "tab": tab, "text": text}),
            PendingMessage::Inbound { label, text } => json!({"op": "inbound", "tab": tab, "label": label, "text": text}),
        }).collect();
        window.following.insert(tab, Follow { window: other, tab: other_tab, session: session.to_string(), pending });
        // First what was said so far: the log may not have the turn in progress yet.
        for mut entry in said {
            entry["tab"] = json!(tab);
            window.say(entry);
        }
        if let Some(mut prompt) = prompt {
            prompt["tab"] = json!(tab);
            window.asking.insert(tab, prompt.clone());
            window.say(prompt);
        }
        Some(json!({"ok": true, "attached": true}))
    }

    /// An operation on a tab showing another window's run.
    fn on_followed(windows: &mut HashMap<String, Window>, id: &str, tab: u64, name: &str, op: &Value) -> Result<Value, String> {
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        let follow = window.following.get_mut(&tab).ok_or("not following")?;
        match name {
            "session_of" => Ok(json!({"session": follow.session})),
            "rebind" => Ok(json!({"ok": false})),
            "task" => Ok(json!({"invalid": "this conversation is finishing a run; give the task when it has ended"})),
            "unqueue" => {
                let at = follow.pending.iter().position(|p| p["op"] == "prompt" && p["text"] == op["text"]);
                if let Some(at) = at {
                    follow.pending.remove(at);
                }
                Ok(json!({"ok": at.is_some()}))
            }
            "answer" | "stop" => {
                let (owner, owner_tab) = (follow.window.clone(), follow.tab);
                window.asking.remove(&tab);
                let owner = windows.get(&owner).ok_or("the run has ended")?;
                if name == "stop" {
                    owner.sessions.stop(owner_tab);
                } else {
                    owner.sessions.answer(owner_tab, answer_from_json(&op["answer"]));
                }
                Ok(json!({"ok": true}))
            }
            "note" | "close" => window.apply(name, op),
            _ => {
                follow.pending.push(op.clone());
                Ok(json!({"ok": true}))
            }
        }
    }

    fn open(&self, windows: &mut HashMap<String, Window>, id: &str, op: &Value) -> Result<Value, String> {
        let root = op["root"].as_str().ok_or("root is required")?.to_string();
        let settings: Settings = serde_json::from_value(op["settings"].clone()).map_err(|e| format!("settings: {e}"))?;
        let default = crate::Choice { provider: settings.provider.clone(), model: settings.model.clone(), slave: false };
        let for_window = settings.clone();
        let (reload, notes, choices) = match windows.get(id) {
            Some(w) => (w.reload.clone(), w.notes.clone(), w.choices.clone()),
            None => (Arc::default(), Arc::default(), Arc::default()),
        };
        let lookup = notes.clone();
        let note: Arc<dyn Fn(u64) -> Option<String> + Send + Sync> = Arc::new(move |tab| lookup.lock().ok()?.get(&tab).cloned());
        let environment: Option<crate::WindowEnv> = serde_json::from_value(op["env"].clone()).ok().map(Arc::new);
        let mut host = (self.make_host)(&settings, note, reload.clone());
        host.environment = environment.clone();
        host.window = Some(id.to_string());
        host.choices = choices.clone();
        let factory = crate::with_host(settings, root.clone(), self.data_dir.clone(), host);
        match windows.get_mut(id) {
            Some(window) => window.sessions.set_factory(factory),
            None => {
                let sessions = Sessions::new(factory, root.clone()).with_data_dir(&self.data_dir.to_string_lossy());
                windows.insert(id.to_string(), Window { sessions, root, events: VecDeque::new(), next: 0, reload, notes, choices, default: default.clone(), settings: for_window.clone(), slave_asks: Default::default(), program_asks: Vec::new(), program_allowed: Default::default(), polled: Instant::now(), closed: false, asking: HashMap::new(), plan_asks: HashMap::new(), handed: HashMap::new(), following: HashMap::new(), said: HashMap::new(), stopped_starting: false, environment: None });
            }
        }
        if let Some(window) = windows.get_mut(id) {
            window.environment = environment;
            window.default = default;
            window.settings = for_window;
        }
        Ok(json!({"ok": true, "pid": std::process::id()}))
    }
}

impl ConversationHost for DaemonHost {
    fn open_sessions(&self) -> Vec<harness_daemon::OpenSession> {
        let windows = self.windows.lock().expect("windows");
        let data = self.data_dir.to_string_lossy();
        let mut out = Vec::new();
        for window in windows.values().filter(|w| !w.closed) {
            for (_, session) in window.sessions.open() {
                if !out.iter().any(|s: &harness_daemon::OpenSession| s.id == session) {
                    let nick = self.nicks.lock().expect("nicks").get(&session).cloned();
                    let title = nick.clone().unwrap_or_else(|| crate::title_of(&data, &window.root, &session));
                    out.push(harness_daemon::OpenSession { title, nick, id: session });
                }
            }
        }
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    fn deliver_to(&self, session: &str, label: &str, text: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.inbound(tab, label.to_string(), text.to_string());
                self.wake.notify_all();
                return true;
            }
        }
        false
    }

    fn notify(&self, session: &str, label: &str, text: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.notice(tab, label.to_string(), text.to_string());
                self.wake.notify_all();
                return true;
            }
        }
        false
    }

    fn approve_plan(&self, session: &str, yes: bool) -> Result<String, String> {
        let mut windows = self.windows.lock().expect("windows");
        let window = windows.values_mut().filter(|w| !w.closed).find(|w| w.sessions.open().iter().any(|(_, s)| s == session)).ok_or("that session is not open any more")?;
        let tab = window.sessions.open().into_iter().find(|(_, s)| s == session).map(|(tab, _)| tab).ok_or("that session is not open any more")?;
        // A delegate's card (CON-163): the press decides, and the card goes from the window.
        if let Some(ask) = window.plan_asks.get(&tab).filter(|ask| ask.session == session).cloned() {
            window.plan_asks.remove(&tab);
            window.say(json!({"kind": "ask_withdrawn", "tab": tab, "call_id": ask.call_id, "approved": yes}));
            self.decide_plan(window, tab, session, yes, true);
        // The native loop's own card: its plan_exit gets the answer, and the card goes.
        } else if let Some(asked) = window.asking.get(&tab).filter(|asked| is_plan_ask(asked)).cloned() {
            window.asking.remove(&tab);
            window.sessions.answer(tab, if yes { Answer::AllowOnce } else { Answer::Deny });
            window.say(json!({"kind": "ask_withdrawn", "tab": tab, "call_id": asked["call"]["id"], "approved": yes}));
        // Planning with no card (a daemon restarted since it asked): as CON-161 did.
        } else if self.orchestra.lock().expect("orchestra").planning.contains(session) {
            self.decide_plan(window, tab, session, yes, true);
        } else {
            return Err("уже решено: этот план больше не ждёт одобрения".to_string());
        }
        self.wake.notify_all();
        Ok(if yes { "✅ Одобрено в Telegram: агент выходит из режима плана.".to_string() } else { "✏️ Не одобрено в Telegram: агент спросит, что изменить.".to_string() })
    }

    fn remind(&self, session: &str, hook: &str, text: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.remind(tab, hook.to_string(), text.to_string());
                self.wake.notify_all();
                return true;
            }
        }
        false
    }

    fn session_of(&self, window: &str, tab: u64) -> Option<String> {
        self.windows.lock().expect("windows").get(window)?.sessions.session_of(tab).map(str::to_string)
    }

    fn rename(&self, session: &str, nick: &str) -> bool {
        let mut windows = self.windows.lock().expect("windows");
        for window in windows.values_mut().filter(|w| !w.closed) {
            if let Some((tab, _)) = window.sessions.open().into_iter().find(|(_, s)| s == session) {
                window.sessions.nick(tab, nick.to_string());
                return true;
            }
        }
        false
    }

    fn close(&self, only_if_idle: bool) -> bool {
        let threads: Vec<_> = {
            // Under the lock every operation takes: no message starts a run between the look and the close.
            let mut windows = self.windows.lock().expect("windows");
            if only_if_idle && windows.values().any(|w| w.sessions.busy()) {
                return false;
            }
            windows.drain().flat_map(|(_, mut window)| window.sessions.close()).collect()
        };
        self.wake.notify_all();
        let end = Instant::now() + CLOSING;
        while Instant::now() < end && !threads.iter().all(|t| t.is_finished()) {
            std::thread::sleep(PUMP);
        }
        true
    }

    fn op(&self, id: &str, op: &Value) -> Result<Value, String> {
        let mut windows = self.windows.lock().expect("windows");
        let name = op["op"].as_str().ok_or("op is required")?;
        if name == "open" {
            return self.open(&mut windows, id, op);
        }
        if name == "busy" {
            return Ok(json!({"busy": windows.values().any(|w| w.sessions.busy())}));
        }
        // Whether this window has a run on in the daemon, which it may not have heard of yet.
        if name == "window_busy" {
            return Ok(json!({"busy": windows.get(id).is_some_and(|w| w.sessions.busy())}));
        }
        // A debug call: the window the user is most likely looking at draws itself into a file.
        if name == "screenshot" {
            let path = op["text"].as_str().ok_or("text is required: the file to write")?;
            let window = windows.values_mut().filter(|w| !w.silent()).max_by_key(|w| w.polled).ok_or("no window is open")?;
            window.say(json!({"kind": "screenshot", "path": path}));
            self.wake.notify_all();
            return Ok(json!({"ok": true}));
        }
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        window.polled = Instant::now();
        let tab = op["tab"].as_u64().unwrap_or(0);
        // An agent asks for the harness to be rebuilt: the window does it, and tells that tab.
        if name == "reload" {
            // It would restart everyone: a slave asks its master.
            if window.sessions.session_of(tab).is_some_and(|session| self.orchestra.lock().expect("orchestra").slaves.contains_key(session)) {
                return Err("rebuilding restarts every agent: ask your orchestrator to do it".to_string());
            }
            window.say(json!({"kind": "reload", "tab": tab}));
            self.wake.notify_all();
            return Ok(json!({"ok": true}));
        }
        if name == "left" {
            let waiting = self.left.lock().expect("left").remove(op["text"].as_str().unwrap_or_default()).unwrap_or_default();
            return Ok(json!({"messages": waiting}));
        }
        if name == "choose" {
            // An agent stays one whatever it is moved to (CON-108).
            let slave = window.choices.lock().expect("choices").get(&tab).is_some_and(|choice| choice.slave);
            let choice = crate::Choice { provider: op["provider"].as_str().ok_or("provider is required")?.to_string(), model: op["model"].as_str().ok_or("model is required")?.to_string(), slave };
            return self.choose(window, tab, choice);
        }
        if name == "choice_of" {
            return Ok(json!({"choice": window.choices.lock().expect("choices").get(&tab)}));
        }
        if name == "adopt" {
            let reply = self.adopt(window, op);
            self.wake.notify_all();
            return reply;
        }
        if matches!(name, "mode__plan_enter" | "mode__plan_exit" | "mode__debug") {
            let reply = self.delegate_mode_op(window, tab, name, op);
            self.wake.notify_all();
            return reply;
        }
        if name.starts_with("mode__") {
            let reply = self.orchestra_op(&mut windows, id, tab, name, op).unwrap_or_else(|| Err(format!("no mode tool {name}")));
            self.wake.notify_all();
            return reply;
        }
        if name == "slave_task" {
            let task = window.sessions.session_of(tab).and_then(|session| self.orchestra.lock().expect("orchestra").tasks.get(session).copied());
            return Ok(json!({"task": task}));
        }
        if name == "slave_ask" {
            let (tool, input) = (op["tool"].as_str().unwrap_or_default(), &op["input"]);
            let slave = window.sessions.session_of(tab).map(str::to_string).filter(|session| self.orchestra.lock().expect("orchestra").slaves.contains_key(session));
            // The user's own tab: its program's request is the user's, asked in that tab as the
            // native loop's are (journal#814). Answered through the tab's answer, below.
            let Some(session) = slave else {
                let key = ask_key(tool, input);
                if window.program_allowed.contains(&(tab, key.clone())) {
                    return Ok(json!({"answer": "always"}));
                }
                let id = self.next_ask.fetch_add(1, Ordering::SeqCst);
                window.program_asks.push(ProgramAsk { id, tab, key, answer: None });
                let call = harness_api::ToolCall { id: format!("program-ask-{id}"), name: tool.to_string(), input: input.clone() };
                window.say(json!({"kind": "ask", "tab": tab, "call": call, "capabilities": []}));
                self.wake.notify_all();
                return Ok(json!({"id": id, "user": true}));
            };
            let planning = self.orchestra.lock().expect("orchestra").planning.contains(&session);
            // An edit by script is answered here: the file tools do it with no one asked (journal#674).
            if tool == "Bash"
                && let harness_core::plan::ScriptEdit::Refused(why) = harness_core::plan::script_edit(input["command"].as_str().unwrap_or_default())
            {
                return Ok(json!({"refused": why}));
            }
            let key = ask_key(tool, input);
            if program_may(tool, input, &window.root, planning) || window.program_allowed.contains(&(tab, key.clone())) {
                return Ok(json!({"answer": "always"}));
            }
            let id = self.next_ask.fetch_add(1, Ordering::SeqCst);
            window.program_asks.push(ProgramAsk { id, tab, key, answer: None });
            self.program_words.lock().expect("words").push((session, format!("asks your permission: {tool} {}\nAnswer with mode__agent_answer: once, always or deny.", brief_input(input))));
            self.wake.notify_all();
            return Ok(json!({"id": id}));
        }
        if name == "slave_asked" {
            let id = op["id"].as_u64().ok_or("id is required")?;
            let Some(at) = window.program_asks.iter().position(|ask| ask.id == id) else { return Ok(json!({"answer": "deny"})) };
            return Ok(match window.program_asks[at].answer.clone() {
                Some(answer) => {
                    window.program_asks.remove(at);
                    json!({"answer": answer})
                }
                None => json!({"waiting": true}),
            });
        }
        if name == "agent_list" {
            let asking = window.sessions.session_of(tab).map(str::to_string);
            return Ok(self.agent_list(&windows, asking.as_deref()));
        }
        if name == "agent_send" {
            let reply = self.agent_send(&mut windows, id, tab, op);
            self.wake.notify_all();
            return reply;
        }
        if name == "resume" {
            let session = op["text"].as_str().unwrap_or_default();
            self.settle(window, tab, session);
            if let Some(reply) = Self::take_over(&mut windows, id, tab, session) {
                self.wake.notify_all();
                return Ok(reply);
            }
        } else if window.following.contains_key(&tab) {
            return Self::on_followed(&mut windows, id, tab, name, op);
        }
        let window = windows.get_mut(id).ok_or("this window is not open here")?;
        // The answer to a plan's card (CON-163): it decides here, and Telegram's message follows.
        if name == "answer"
            && let Some(ask) = window.plan_asks.remove(&tab)
        {
            let yes = !matches!(answer_from_json(&op["answer"]), Answer::Deny);
            self.decide_plan(window, tab, &ask.session, yes, false);
            self.plan_done(&ask.session, yes);
            self.wake.notify_all();
            return Ok(json!({"ok": true}));
        }
        // The native loop's plan answered here: Telegram's message follows (CON-163).
        if name == "answer"
            && window.asking.get(&tab).is_some_and(is_plan_ask)
            && let Some(session) = window.sessions.session_of(tab).map(str::to_string)
        {
            self.plan_done(&session, !matches!(answer_from_json(&op["answer"]), Answer::Deny));
        }
        // An answer to the program's own request (journal#814) is that request's, not the loop's.
        if name == "answer" && window.sessions.session_of(tab).is_some() && !window.sessions.running(tab) {
            // What a program that has stopped asked is no one's to answer now.
            window.program_asks.retain(|ask| ask.tab != tab);
        }
        if name == "answer"
            && let Some(ask) = window.program_asks.iter_mut().find(|ask| ask.tab == tab && ask.answer.is_none())
        {
            let answer = match answer_from_json(&op["answer"]) {
                Answer::AllowOnce => "once",
                Answer::AlwaysAllow => "always",
                Answer::Deny => "deny",
            };
            if answer == "always" {
                let key = ask.key.clone();
                window.program_allowed.insert((tab, key));
            }
            if let Some(ask) = window.program_asks.iter_mut().find(|ask| ask.tab == tab && ask.answer.is_none()) {
                ask.answer = Some(answer.to_string());
            }
            return Ok(json!({"ok": true}));
        }
        let reply = window.apply(name, op);
        // A first message made it a conversation.
        if let Some(session) = window.sessions.session_of(tab).map(str::to_string) {
            self.settle(window, tab, &session);
        }
        reply
    }

    fn events(&self, id: &str, after: u64, wait: Duration) -> Vec<Value> {
        let deadline = Instant::now() + wait;
        let mut windows = self.windows.lock().expect("windows");
        loop {
            let Some(window) = windows.get_mut(id) else { return Vec::new() };
            window.polled = Instant::now();
            while window.events.front().is_some_and(|(seq, _)| *seq <= after) {
                window.events.pop_front();
            }
            if !window.events.is_empty() {
                return window.events.iter().map(|(seq, event)| json!({"seq": seq, "event": event})).collect();
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Vec::new();
            }
            windows = self.wake.wait_timeout(windows, left).expect("windows").0;
        }
    }

    fn stats(&self) -> Value {
        let windows = self.windows.lock().expect("windows");
        json!({"windows": windows.len(), "busy": windows.values().filter(|w| w.sessions.busy()).count(), "roots": windows.values().map(|w| w.root.clone()).collect::<HashSet<_>>()})
    }
}

/// The client's side: the same operations as `Sessions`, all of them answered by the daemon.
pub struct RemoteSessions {
    link: Arc<Mutex<Link>>,
    window: String,
    incoming: Receiver<FromSession>,
    tabs: Arc<Mutex<HashSet<u64>>>,
    stopped: Arc<AtomicBool>,
    say: Sender<FromSession>,
    /// The tab whose agent asked for a reload.
    asked_by: Arc<std::sync::atomic::AtomicU64>,
    /// Where a debug call has asked this window to draw itself.
    shot: Arc<Mutex<Option<String>>>,
}

/// The daemon a window speaks to, and what it takes to open the window again in another one: a
/// daemon of a newer build replaces an idle one, and the windows of the old one go on (journal#807).
struct Link {
    client: Client,
    /// The window's last `open`: its root, settings and environment.
    open: Value,
    /// The conversation each tab holds, to resume it in the new daemon.
    sessions: std::collections::BTreeMap<u64, String>,
    /// How many times the window moved to another daemon.
    generation: u64,
}

/// Opens the window again in a daemon that is new, with its tabs' conversations resumed (CON-131):
/// the client runs it on every connection to a daemon of another pid, before what waits is sent.
fn reopen(link: &Mutex<Link>, client: &Client, window: &str) {
    let (mut open, sessions) = {
        let link = link.lock().expect("link");
        (link.open.clone(), link.sessions.clone())
    };
    open["window"] = json!(window);
    open["op"] = json!("open");
    if client.conversation_op(&open).is_err() {
        return;
    }
    for (tab, session) in &sessions {
        let _ = client.conversation_op(&json!({"window": window, "op": "resume", "tab": tab, "text": session}));
    }
    link.lock().expect("link").generation += 1;
}

/// One operation through `link`'s client, which waits while the daemon is away (CON-131).
fn op_through(link: &Mutex<Link>, _window: &str, op: &Value) -> Result<Value, String> {
    let client = link.lock().expect("link").client.clone();
    client.conversation_op(op)
}

impl RemoteSessions {
    /// Opens this window in the daemon and starts listening. `reload` is raised when a session
    /// asks for a hot reload.
    pub fn open(client: Client, root: &str, settings: &Settings, reload: Arc<AtomicBool>) -> Result<RemoteSessions, String> {
        Self::open_in(client, root, settings, reload, crate::this_environment())
    }

    /// The same, for a window with this environment rather than this process's.
    pub fn open_in(client: Client, root: &str, settings: &Settings, reload: Arc<AtomicBool>, environment: std::collections::BTreeMap<String, String>) -> Result<RemoteSessions, String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let window = format!("conv-{}-{now}", std::process::id());
        let open = json!({"root": root, "settings": settings, "env": environment});
        // Not connected yet (a daemon of another protocol is waited for): the window opens itself
        // there from its hook once the connection is made.
        if client.current() {
            client.conversation_op(&json!({"window": window, "op": "open", "root": root, "settings": settings, "env": environment}))?;
        }
        let link = Arc::new(Mutex::new(Link { client: client.clone(), open, sessions: Default::default(), generation: 0 }));
        {
            let (weak, window) = (Arc::downgrade(&link), window.clone());
            client.on_connect(Arc::new(move |client: &Client, new_daemon: bool| {
                if let (true, Some(link)) = (new_daemon, weak.upgrade()) {
                    reopen(&link, client, &window);
                }
            }));
        }
        let (sender, incoming) = channel();
        let (tabs, stopped): (Arc<Mutex<HashSet<u64>>>, Arc<AtomicBool>) = (Arc::default(), Arc::default());
        let asked_by: Arc<std::sync::atomic::AtomicU64> = Arc::default();
        let shot: Arc<Mutex<Option<String>>> = Arc::default();
        listen(link.clone(), window.clone(), sender.clone(), tabs.clone(), stopped.clone(), Asks { reload, asked_by: asked_by.clone(), shot: shot.clone() });
        Ok(RemoteSessions { link, window, incoming, tabs, stopped, say: sender, asked_by, shot })
    }

    /// One operation of this window, through whichever daemon now answers.
    fn op(&self, op: &Value) -> Result<Value, String> {
        op_through(&self.link, &self.window, op)
    }

    fn call(&self, op: &str, tab: u64, mut rest: Value) -> Option<Value> {
        rest["window"] = json!(self.window);
        rest["op"] = json!(op);
        rest["tab"] = json!(tab);
        self.tabs.lock().expect("tabs").insert(tab);
        if op == "resume"
            && let Some(session) = rest["text"].as_str()
        {
            self.link.lock().expect("link").sessions.insert(tab, session.to_string());
        }
        let reply = self.op(&rest);
        // A daemon that cannot be reached is said on the tab, never left as a silent wait.
        if let Err(error) = &reply
            && matches!(op, "prompt" | "task" | "inbound" | "answer")
            && !self.link.lock().expect("link").client.current()
        {
            let _ = self.say.send(FromSession::Unavailable { tab, message: format!("The daemon that runs this conversation cannot be reached ({error}). The log keeps it: open it again from the start page.") });
        }
        reply.ok()
    }

    pub fn configure(&self, root: &str, settings: &Settings) -> Result<(), String> {
        let environment = self.link.lock().expect("link").open["env"].clone();
        let open = json!({"root": root, "settings": settings, "env": environment});
        self.op(&json!({"window": self.window, "op": "open", "root": root, "settings": settings, "env": environment}))?;
        self.link.lock().expect("link").open = open;
        Ok(())
    }

    /// The environment this window's sessions start with from the next `configure` on: another
    /// profile's (CON-121). Sessions already started keep the one they began with.
    pub fn set_environment(&mut self, environment: std::collections::BTreeMap<String, String>) {
        self.link.lock().expect("link").open["env"] = json!(environment);
    }
}

impl Drop for RemoteSessions {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let client = self.link.lock().expect("link").client.clone();
        let _ = client.conversation_op(&json!({"window": self.window, "op": "close"}));
    }
}

/// What the daemon can ask of this window beyond its conversations.
struct Asks {
    reload: Arc<AtomicBool>,
    asked_by: Arc<std::sync::atomic::AtomicU64>,
    shot: Arc<Mutex<Option<String>>>,
}

/// Hears the window's conversation events, pushed by the daemon on the window's connection
/// (CON-130). The client subscribes again by itself after a loss, from where it was, or from the
/// start in a new daemon, which numbers them afresh.
fn listen(link: Arc<Mutex<Link>>, window: String, sender: Sender<FromSession>, _tabs: Arc<Mutex<HashSet<u64>>>, stopped: Arc<AtomicBool>, asks: Asks) {
    let Asks { reload, asked_by, shot } = asks;
    std::thread::spawn(move || {
        let client = link.lock().expect("link").client.clone();
        let batches = client.subscribe("conversations", &window, 0);
        for events in batches {
            if stopped.load(Ordering::SeqCst) {
                return;
            }
            for event in events {
                if event["event"]["kind"] == "screenshot" {
                    *shot.lock().expect("shot") = event["event"]["path"].as_str().map(str::to_string);
                } else if event["event"]["kind"] == "reload" {
                    // Which tab asked, if the asking came from outside this process.
                    if let Some(tab) = event["event"]["tab"].as_u64() {
                        asked_by.store(tab, Ordering::SeqCst);
                    }
                    reload.store(true, Ordering::SeqCst);
                } else if let Some(message) = from_wire(&event["event"]) {
                    // The conversation a tab holds, kept so another daemon can resume it.
                    let tab = tab_of(&message);
                    if matches!(message, FromSession::Entry { .. }) && !link.lock().expect("link").sessions.contains_key(&tab)
                        && let Ok(reply) = client.conversation_op(&json!({"window": window, "op": "session_of", "tab": tab}))
                        && let Some(session) = reply["session"].as_str()
                    {
                        link.lock().expect("link").sessions.insert(tab, session.to_string());
                    }
                    if sender.send(message).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// What opening a stored session came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opened {
    /// The tab has it; its next message continues the log.
    Ready,
    /// Its run is on, and now speaks to this tab; what it says next comes as for any run.
    Attached,
    /// Another window has it open: said on the tab.
    Elsewhere,
}

/// What a frontend holds. `Local` is for scripted runs and for when no daemon can be had.
pub enum SessionLink {
    Local(Sessions),
    Remote(RemoteSessions),
}

impl SessionLink {
    /// The window's connection to the daemon, when its conversations run there.
    pub fn client(&self) -> Option<Client> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => Some(r.link.lock().expect("link").client.clone()),
        }
    }

    /// The daemon's id of this window, which its delegates' MCP servers are given.
    pub fn window_id(&self) -> Option<&str> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => Some(&r.window),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, SessionLink::Remote(_))
    }

    /// Moves the window onto another environment for the sessions it starts from now on. Only a
    /// window served by the daemon can: one running its own sessions reads this process's.
    pub fn set_environment(&mut self, environment: std::collections::BTreeMap<String, String>) -> Result<(), String> {
        match self {
            // A window that runs its sessions itself gives the environment to its own factory, through
            // the Host it builds (journal#824): nothing to tell a daemon here.
            SessionLink::Local(_) => Ok(()),
            SessionLink::Remote(r) => {
                r.set_environment(environment);
                Ok(())
            }
        }
    }

    pub fn configure(&mut self, root: &str, settings: &Settings, local: impl FnOnce() -> Factory) -> Result<(), String> {
        match self {
            SessionLink::Local(s) => {
                if s.busy() {
                    return Err("a run is on: the adapter can be changed when it ends".to_string());
                }
                s.set_factory(local());
                Ok(())
            }
            SessionLink::Remote(r) => r.configure(root, settings),
        }
    }

    pub fn note(&self, tab: u64, text: String) {
        if let SessionLink::Remote(r) = self {
            r.call("note", tab, json!({"text": text}));
        }
    }

    pub fn resume(&mut self, tab: u64, session: &str) -> Opened {
        match self {
            SessionLink::Local(s) => {
                s.resume(tab, session);
                Opened::Ready
            }
            SessionLink::Remote(r) => {
                // The other window may have just gone: its silence shows within seconds.
                let deadline = Instant::now() + WAIT_FOR_OTHER;
                loop {
                    let Some(reply) = r.call("resume", tab, json!({"text": session})) else { return Opened::Ready };
                    if reply["attached"] == true {
                        return Opened::Attached;
                    }
                    if reply["elsewhere"] != true {
                        return Opened::Ready;
                    }
                    if Instant::now() >= deadline {
                        let _ = r.say.send(FromSession::Unavailable { tab, message: "This conversation is open in another window. Close it there to go on here: one writer to a log.".to_string() });
                        return Opened::Elsewhere;
                    }
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
        }
    }

    /// What was sent to this session from a window that went away before it could go in.
    pub fn left_waiting(&self, tab: u64, session: &str) -> Vec<PendingMessage> {
        match self {
            SessionLink::Local(_) => Vec::new(),
            SessionLink::Remote(r) => r.call("left", tab, json!({"text": session})).map_or(Vec::new(), |reply| serde_json::from_value(reply["messages"].clone()).unwrap_or_default()),
        }
    }

    /// The tab whose agent asked for the reload in hand, if one did from outside this process.
    /// The file a debug call wants this window drawn into, once.
    pub fn screenshot_asked(&self) -> Option<String> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => r.shot.lock().expect("shot").take(),
        }
    }

    pub fn reload_asked_by(&self) -> Option<u64> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => Some(r.asked_by.swap(0, Ordering::SeqCst)).filter(|tab| *tab != 0),
        }
    }

    pub fn session_of(&self, tab: u64) -> Option<String> {
        match self {
            SessionLink::Local(s) => s.session_of(tab).map(str::to_string),
            SessionLink::Remote(r) => r.call("session_of", tab, json!({}))?["session"].as_str().map(str::to_string),
        }
    }

    pub fn inbound(&mut self, tab: u64, label: String, text: String) {
        match self {
            SessionLink::Local(s) => s.inbound(tab, label, text),
            SessionLink::Remote(r) => {
                r.call("inbound", tab, json!({"label": label, "text": text}));
            }
        }
    }

    /// This tab's conversation goes on on this adapter, and no other moves. With no daemon there is
    /// no one to keep it: the tab takes the settings as they are now, like any tab started after.
    pub fn choose(&mut self, tab: u64, provider: &str, model: &str) -> Result<bool, String> {
        match self {
            SessionLink::Local(s) => Ok(s.rebind(tab)),
            SessionLink::Remote(r) => r.op(&json!({"window": r.window, "op": "choose", "tab": tab, "provider": provider, "model": model})).map(|reply| reply["ok"] == true),
        }
    }

    /// A window has opened a new agent's tab.
    pub fn adopt(&mut self, tab: u64, master_tab: u64, nick: &str, provider: &str, model: &str, task: u64) -> Result<(), String> {
        match self {
            SessionLink::Local(_) => Err("an orchestra needs the daemon".to_string()),
            SessionLink::Remote(r) => r.op(&json!({"window": r.window, "op": "adopt", "tab": tab, "master_tab": master_tab, "nick": nick, "provider": provider, "model": model, "task": task})).map(|_| ()),
        }
    }

    /// What this tab's conversation runs on, if the daemon has settled it.
    pub fn choice_of(&self, tab: u64) -> Option<crate::Choice> {
        match self {
            SessionLink::Local(_) => None,
            SessionLink::Remote(r) => r.op(&json!({"window": r.window, "op": "choice_of", "tab": tab})).ok().and_then(|reply| serde_json::from_value(reply["choice"].clone()).ok()),
        }
    }

    pub fn rebind(&mut self, tab: u64) -> bool {
        match self {
            SessionLink::Local(s) => s.rebind(tab),
            SessionLink::Remote(r) => r.call("rebind", tab, json!({})).is_none_or(|reply| reply["ok"] == true),
        }
    }

    pub fn prompt(&mut self, tab: u64, text: String) {
        match self {
            SessionLink::Local(s) => s.prompt(tab, text),
            SessionLink::Remote(r) => {
                r.call("prompt", tab, json!({"text": text}));
            }
        }
    }

    pub fn task(&mut self, tab: u64, text: String, steps: Vec<String>) -> Result<(), TaskRefused> {
        match self {
            SessionLink::Local(s) => s.task(tab, text, steps),
            SessionLink::Remote(r) => {
                let reply = r.call("task", tab, json!({"text": text, "steps": steps})).ok_or_else(|| TaskRefused::Invalid("the daemon did not answer".to_string()))?;
                if let Some(reason) = reply["invalid"].as_str() {
                    return Err(TaskRefused::Invalid(reason.to_string()));
                }
                if reply["not_approved"].is_object() {
                    let commands = serde_json::from_value(reply["not_approved"]["commands"].clone()).unwrap_or_default();
                    return Err(TaskRefused::NotApproved { commands, text: reply["not_approved"]["text"].as_str().unwrap_or_default().to_string() });
                }
                Ok(())
            }
        }
    }

    pub fn plan(&mut self, tab: u64, on: bool) {
        match self {
            SessionLink::Local(s) => s.plan(tab, on),
            SessionLink::Remote(r) => {
                r.call("plan", tab, json!({"on": on}));
            }
        }
    }

    pub fn step_debug(&mut self, tab: u64, on: bool) {
        match self {
            SessionLink::Local(s) => s.step_debug(tab, on),
            SessionLink::Remote(r) => {
                r.call("step_debug", tab, json!({"on": on}));
            }
        }
    }

    pub fn nick(&mut self, tab: u64, name: String) {
        match self {
            SessionLink::Local(s) => s.nick(tab, name),
            SessionLink::Remote(r) => {
                r.call("nick", tab, json!({"text": name}));
            }
        }
    }

    pub fn unqueue(&mut self, tab: u64, text: &str) -> bool {
        match self {
            SessionLink::Local(s) => s.unqueue(tab, text),
            SessionLink::Remote(r) => r.call("unqueue", tab, json!({"text": text})).is_some_and(|reply| reply["ok"] == true),
        }
    }

    pub fn answer(&self, tab: u64, answer: Answer) {
        match self {
            SessionLink::Local(s) => s.answer(tab, answer),
            SessionLink::Remote(r) => {
                r.call("answer", tab, json!({"answer": answer_to_json(answer)}));
            }
        }
    }

    pub fn stop(&self, tab: u64) {
        match self {
            SessionLink::Local(s) => s.stop(tab),
            SessionLink::Remote(r) => {
                r.call("stop", tab, json!({}));
            }
        }
    }

    pub fn drain(&self) -> Vec<FromSession> {
        match self {
            SessionLink::Local(s) => s.drain(),
            SessionLink::Remote(r) => r.incoming.try_iter().collect(),
        }
    }
}

/// The daemon for this window (CON-130): any daemon that speaks this protocol, whatever its build,
/// found or started. One of another protocol, such as a build from before the socket, is stopped
/// when it is idle and this build's started; while it runs other windows' conversations it is left
/// alone, and the error says so.
pub fn daemon_for_this_build(data_dir: &Path, program: &Path) -> Result<Client, String> {
    if let Some(old) = harness_daemon::client::other_protocol(data_dir) {
        if !harness_daemon::client::stop_if_idle(&old) {
            // Busy with other windows' runs: this window waits for it, connected to nothing yet; its
            // client asks it to stop every 2 s and starts this build's once it has (CON-130).
            return Ok(Client::for_window(data_dir, program));
        }
        let end = Instant::now() + Duration::from_secs(5);
        while harness_daemon::client::other_answers(&old) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Client::find_or_start(data_dir, program)
}

/// The adapters of these settings that are authorized here, with the models known for each.
/// They are the window's: its environment holds its profile's keys and credential folder (CON-122).
fn authorized_adapters(settings: &Settings, data_dir: &Path, environment: Option<crate::WindowEnv>) -> Vec<Value> {
    let read = crate::env_reader(environment);
    let keys = harness_auth_apikey::ApiKeyAuth::new({ let read = read.clone(); move |name| read(name) }, &json!(settings.auth));
    let names = std::iter::once(settings.provider.as_str()).chain(settings.providers.keys().map(String::as_str));
    let mut seen = Vec::new();
    let mut rows = Vec::new();
    for name in names {
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let row = settings.providers.get(name);
        let is_delegate = row.is_some_and(crate::is_delegate_row);
        let on_plan = name == "chatgpt" || settings.auth.get(name).is_some_and(|a| a["oauth"] == "chatgpt");
        let authorized = if is_delegate {
            true
        } else if on_plan {
            harness_auth_chatgpt::status(&crate::chatgpt_store(&|name: &str| read(name), data_dir)).0
        } else {
            let source = keys.source(name);
            !source.starts_with("not set")
        };
        if !authorized {
            continue;
        }
        let mut models: Vec<String> = row.and_then(|r| r["models"].as_array()).map_or_else(Vec::new, |list| list.iter().filter_map(|m| m.as_str().map(str::to_string)).collect());
        if name == settings.provider && !models.contains(&settings.model) {
            models.push(settings.model.clone());
        }
        rows.push(json!({"adapter": name, "models": models, "note": "any model this adapter takes may be named"}));
    }
    rows
}

/// A call's arguments with each long text cut and none dropped: the master checks the short ones
/// that come after a long one, such as `user_said` and `proof` (W-3, W-42).
fn brief_input(input: &Value) -> String {
    const FIELD: usize = 200;
    fn cut(value: &Value) -> Value {
        match value {
            Value::String(text) if text.chars().count() > FIELD => Value::String(text.chars().take(FIELD).collect::<String>() + "…"),
            Value::Array(items) => Value::Array(items.iter().map(cut).collect()),
            Value::Object(fields) => Value::Object(fields.iter().map(|(key, value)| (key.clone(), cut(value))).collect()),
            other => other.clone(),
        }
    }
    cut(input).to_string().replace('\n', " ")
}

/// What an agent has done, line by line, for its master to check it by.
fn read_agent(data_dir: &str, root: &str, session: &str, live: &[Value]) -> Vec<String> {
    const LIMIT: usize = 300;
    let short = |text: &str| {
        let text: String = text.chars().take(300).collect();
        text.replace('\n', " ")
    };
    let mut lines = Vec::new();
    let mut said = String::new();
    let flush = |said: &mut String, lines: &mut Vec<String>| {
        if !said.trim().is_empty() {
            lines.push(format!("said: {}", short(said)));
        }
        said.clear();
    };
    let mut log = crate::read_log(data_dir, root, session);
    let seen = log.last().map_or(0, |entry| entry.seq);
    log.extend(live.iter().filter_map(|said| harness_api::event::from_line(said["line"].as_str()?).ok()).filter(|entry| entry.seq > seen));
    for entry in &log {
        match &entry.event {
            harness_api::Event::TextDelta { text } => said.push_str(text),
            harness_api::Event::UserMessage { text } => {
                flush(&mut said, &mut lines);
                lines.push(format!("user: {}", short(text)));
            }
            harness_api::Event::Inbound { label, text } => {
                flush(&mut said, &mut lines);
                lines.push(format!("inbound {label}: {}", short(text)));
            }
            harness_api::Event::ToolRequested { call, .. } => {
                flush(&mut said, &mut lines);
                lines.push(format!("call {}: {}", call.name, brief_input(&call.input)));
            }
            harness_api::Event::ToolResult { output, is_error: true, .. } => lines.push(format!("failed: {}", short(output))),
            harness_api::Event::Error { source, message } => lines.push(format!("{RUN_ERROR}{source}: {}", short(message))),
            _ => {}
        }
    }
    flush(&mut said, &mut lines);
    let skip = lines.len().saturating_sub(LIMIT);
    lines.split_off(skip)
}

const RUN_ERROR: &str = "error ";

/// What a master is told of an agent's run that ended with no word to it. An error it ended on comes
/// first: its last words may be long past (journal#671).
fn run_end_word(lines: &[String]) -> String {
    let words = lines.iter().rev().find_map(|line| line.strip_prefix("said: ")).unwrap_or_default();
    match lines.last().and_then(|line| line.strip_prefix(RUN_ERROR)) {
        Some(error) => format!("its run ended on an error, {error}. Read what stands with mode__agent_read; if it cannot go on, close it and start another on its entry. Its last words: {words}"),
        None => format!("its run ended, without a message to you. Its last words: {words}"),
    }
}

#[cfg(test)]
mod read_agent_tests {
    use super::*;

    /// journal#561: a master reading its agent mid-run sees the run so far, which reaches the log
    /// only when the run ends; and nothing twice once it has.
    #[test]
    fn a_read_mid_run_shows_the_run_so_far() {
        let data = std::env::temp_dir().join(format!("mt-read-agent-{}", std::process::id()));
        let line = |seq: u64, event: harness_api::Event| json!({"line": harness_api::event::to_line(&harness_api::LogEntry { seq, at_ms: 0, event })});
        let call = harness_api::ToolCall { id: "c1".into(), name: "Write".into(), input: json!({"file_path": "a.txt"}) };
        let live = vec![
            line(1, harness_api::Event::UserMessage { text: "build it".into() }),
            line(2, harness_api::Event::ToolRequested { call, capabilities: vec![] }),
            line(3, harness_api::Event::TextDelta { text: "writing".into() }),
        ];
        let read = read_agent(&data.to_string_lossy(), "/r", "s1", &live);
        assert_eq!(read, ["user: build it", "call Write: {\"file_path\":\"a.txt\"}", "said: writing"].map(String::from).to_vec(), "{read:?}");
        let _ = std::fs::remove_dir_all(&data);
    }

    /// CON-144: a move's progress crosses the daemon's socket whole, and to the tab it is for.
    #[test]
    fn progress_crosses_the_socket() {
        use harness_api::platform::progress::{Progress, Unit};
        let sent = FromSession::Progress { tab: 4, progress: Progress { what: "Reading Claude Code's copy".into(), done: 410, total: 1196, unit: Unit::Bytes } };
        let wire: Value = serde_json::from_str(&to_wire(&sent).to_string()).unwrap();
        assert_eq!(wire["kind"], "progress");
        assert_eq!(from_wire(&wire), Some(sent.clone()));
        assert_eq!(tab_of(&sent), 4);
    }

    #[test]
    fn a_master_is_reminded_every_ten_minutes_unless_told_otherwise_for_a_test() {
        assert_eq!(agent_check_every(None), Duration::from_secs(600));
        assert_eq!(agent_check_every(Some("12".into())), Duration::from_secs(12));
        assert_eq!(agent_check_every(Some("soon".into())), Duration::from_secs(600));
    }

    #[test]
    fn a_master_is_told_the_error_a_run_ended_on() {
        let lines = ["said: my plan is sent".to_string(), "error chatgpt: the plan's usage limit is reached".to_string()];
        let word = run_end_word(&lines);
        assert!(word.contains("ended on an error") && word.contains("usage limit") && word.contains("my plan is sent"), "{word}");
        let plain = run_end_word(&lines[..1]);
        assert!(plain.contains("without a message") && !plain.contains("error"), "{plain}");
    }

    /// CON-108: what the harness answers of a slave's program by itself, and what goes to its master.
    #[test]
    fn a_slaves_program_asks_its_master_only_beyond_the_basics() {
        let root = "/p";
        let may = |tool: &str, input: Value, planning: bool| program_may(tool, &input, root, planning);
        assert!(may("mcp__harness__journal_add", json!({}), true));
        assert!(may("mcp__mode__plan_exit", json!({}), true));
        assert!(may("Edit", json!({"file_path": "/p/src/a.rs"}), false));
        assert!(!may("Edit", json!({"file_path": "/p/src/a.rs"}), true), "plan mode writes no code");
        assert!(may("Write", json!({"file_path": "/p/docs/a.md"}), true));
        assert!(may("Write", json!({"file_path": "/p/.harness/debug/a.py"}), true));
        assert!(!may("Edit", json!({"file_path": "/etc/passwd"}), false));
        assert!(!may("Write", json!({"file_path": "/p/../etc/a"}), false));
        assert!(may("Bash", json!({"command": "cargo test -p x"}), false));
        assert!(!may("Bash", json!({"command": "cargo test -p x"}), true), "not a reading command");
        assert!(may("Bash", json!({"command": "git status"}), true));
        assert!(!may("Bash", json!({"command": "python3 x.py"}), false));
        assert!(may("Bash", json!({"command": "cargo test | tail"}), false));
        assert!(!may("Bash", json!({"command": "cargo test | tail"}), true));
        assert!(!may("Bash", json!({"command": "cargo test > f"}), false));
        assert!(!may("WebFetch", json!({"url": "https://x"}), false));
    }

    /// W-3, W-42: what comes after a long field still shows, so a master can check it.
    #[test]
    fn a_long_field_does_not_hide_the_short_ones_after_it() {
        let brief = brief_input(&json!({"plan": "x".repeat(5000), "user_said": "теперь давай реализуем", "proof": [".harness/run.log"]}));
        assert!(brief.contains("теперь давай реализуем") && brief.contains(".harness/run.log"), "{brief}");
        assert!(brief.chars().count() < 400, "{}", brief.chars().count());
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    /// journal#1188: ask_withdrawn carries the answer; a daemon of before, without it, means approved.
    #[test]
    fn ask_withdrawn_carries_its_answer() {
        for approved in [true, false] {
            let wire = to_wire(&FromSession::AskWithdrawn { tab: 3, call_id: "plan-ask-1".into(), approved });
            assert!(matches!(from_wire(&wire), Some(FromSession::AskWithdrawn { tab: 3, call_id, approved: a }) if call_id == "plan-ask-1" && a == approved), "{wire}");
        }
        let old = json!({"kind": "ask_withdrawn", "tab": 3, "call_id": "plan-ask-1"});
        assert!(matches!(from_wire(&old), Some(FromSession::AskWithdrawn { approved: true, .. })));
    }
}
