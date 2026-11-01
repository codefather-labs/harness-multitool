//! IF-0.1, the agent loop, and IF-0.2, the bus: every step is an event that updates the state,
//! reaches the frontend and goes to the session log (INV-5).
//!
//! A model that stops ends a *task* only through the verifier (INV-4). A conversation has no
//! protocol, so a stop there waits for the user: never `done`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use harness_api::event::{DecidedBy, HookOutcome, HookPointKind};
use harness_api::{
    Answer, Capability, Content, Decision, Event, Frontend, HookPoint, JournalEntry, LogEntry, ModelEvent, ModelRequest, Protocol, Route, SessionState, Spent, StopReason, TaskState, ToolCall,
    ToolContext, ToolOutput, ToolSpec,
};

use crate::plan;
use crate::registry::Parts;

/// Asks the loop to stop at its next step. Safe to set from another thread.
/// The tool the engine answers itself: the mode is its own state.
const DEBUG_MODE_TOOL: &str = "debug_mode";
const NICK_TOOL: &str = "set_nick";
const ASK_TOOL: &str = "ask_user";
/// The same call failing the same way, within one user message: said at the first, stopped at
/// the second (CON-68).
const REPEAT_WARN: u32 = 3;
const REPEAT_STOP: u32 = 5;
const MAX_NICK: usize = 32;

/// How many binding entries the check for unchecked specifications looks at.
const UNCHECKED_SCAN: usize = 500;

#[derive(Debug, Clone, Default)]
pub struct StopFlag(Arc<AtomicBool>);

/// What the user sent and the agent has not seen yet, shared with the thread that takes input.
pub type Inbox = Arc<std::sync::Mutex<std::collections::VecDeque<String>>>;

impl StopFlag {
    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn is_set(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

/// How a run of the loop ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// The model stopped and there is no protocol: the next user message continues.
    AwaitingInput,
    Done,
    /// Something failed. Final for a task with a protocol; a conversation can go on.
    Failed(String),
    /// The user stopped it. Final for a task with a protocol; a conversation can go on.
    Abandoned,
}

pub struct Engine {
    /// `ask_user` was called: the run ends after this turn's calls.
    asked: std::cell::Cell<bool>,
    /// How many times each failure has come back in this message.
    failures: RefCell<std::collections::HashMap<String, u32>>,
    /// A `Stop` hook asked the run to go on.
    go_on: std::cell::Cell<bool>,
    inbox: Inbox,
    parts: Parts,
    frontend: Rc<dyn Frontend>,
    session: String,
    root: String,
    state: RefCell<SessionState>,
    seq: Cell<u64>,
    /// Entries seen by the frontend, not yet in the store: a streamed turn is synchronous to
    /// its sink, the store is not.
    unsaved: RefCell<Vec<LogEntry>>,
    started: Cell<bool>,
    started_ms: u64,
    stop: StopFlag,
}

impl Engine {
    pub fn new(parts: Parts, frontend: Rc<dyn Frontend>, session: impl Into<String>, root: impl Into<String>) -> Self {
        let started_ms = parts.platform.clock.now_ms();
        Engine {
            asked: std::cell::Cell::new(false),
            failures: RefCell::default(),
            go_on: std::cell::Cell::new(false),
            inbox: Inbox::default(),
            parts,
            frontend,
            session: session.into(),
            root: root.into(),
            state: RefCell::default(),
            seq: Cell::new(0),
            unsaved: RefCell::default(),
            started: Cell::new(false),
            started_ms,
            stop: StopFlag::default(),
        }
    }

    /// The log of a session in the store, to resume from.
    pub async fn load_log(&self) -> Result<Vec<LogEntry>, String> {
        self.parts.store.load(&self.session).await.map_err(|e| format!("session store: {e}"))
    }

    /// Continues a session from its log: the state is the fold of the entries (INV-5).
    pub fn resume(&self, entries: &[LogEntry]) {
        let mut state = self.state.borrow_mut();
        for entry in entries {
            state.apply(&entry.event);
        }
        drop(state);
        self.seq.set(entries.last().map_or(0, |e| e.seq + 1));
        self.started.set(!entries.is_empty());
        // A log cut off mid-tool has calls without results; the model must get one for each.
        for id in self.unanswered_calls() {
            self.emit(Event::ToolResult { id, output: "interrupted: the session ended before this call finished".into(), is_error: true });
        }
    }

    fn unanswered_calls(&self) -> Vec<String> {
        let state = self.state.borrow();
        let mut open = Vec::new();
        for message in &state.messages {
            for content in &message.content {
                match content {
                    Content::ToolUse { id, .. } => open.push(id.clone()),
                    Content::ToolResult { id, .. } => open.retain(|o| o != id),
                    _ => {}
                }
            }
        }
        open
    }

    pub fn stop_flag(&self) -> StopFlag {
        self.stop.clone()
    }

    /// Messages the user sent while a run lasts: woven in at the next step boundary.
    pub fn with_inbox(mut self, inbox: Inbox) -> Self {
        self.inbox = inbox;
        self
    }

    /// Takes what is queued into the conversation. True if anything came.
    async fn take_queued(&self) -> bool {
        let mut any = false;
        loop {
            let Some(text) = self.inbox.lock().expect("inbox").pop_front() else { return any };
            if let Some((hook, reason)) = self.hooks(HookPoint::UserPromptSubmit { text: &text }).await {
                self.emit(Event::Error { source: hook, message: reason });
                continue;
            }
            self.emit(Event::UserMessage { text });
            any = true;
        }
    }

    /// Counts the failures of these calls. True when one has come back so often that going on
    /// only spends the budget: the run ends and the user sees why.
    fn stuck(&self, calls: &[ToolCall]) -> bool {
        let results: Vec<(String, String)> = match self.state.borrow().messages.last() {
            Some(last) => last.content.iter().filter_map(|c| match c {
                Content::ToolResult { id, output, is_error: true } => Some((id.clone(), output.clone())),
                _ => None,
            }).collect(),
            None => return false,
        };
        let mut worst = (0, String::new());
        for (id, output) in results {
            let Some(call) = calls.iter().find(|c| c.id == id) else { continue };
            let mut failures = self.failures.borrow_mut();
            let count = failures.entry(format!("{}\n{}\n{output}", call.name, call.input)).or_insert(0);
            *count += 1;
            if *count > worst.0 {
                worst = (*count, call.name.clone());
            }
        }
        let (count, name) = worst;
        if count == REPEAT_WARN {
            let line = if self.state.borrow().plan_mode {
                format!("You have made this exact {name} call {count} times. You are in PLAN MODE: the environment is fine, this is refused on purpose, and nothing you tried to write exists. It runs after the user approves the plan. Record the plan with journal_add, mindmap_new and flowchart_new, then call plan_exit.")
            } else {
                format!("You have made this exact {name} call {count} times and it failed the same way each time. It will not work: do something different, or use ask_user.")
            };
            self.emit(Event::HookRan { point: HookPointKind::PostToolUse, hook: "repeat-guard".into(), outcome: HookOutcome::Reminder { lines: vec![line] }, detail: None });
        }
        if count >= REPEAT_STOP {
            self.emit(Event::Error { source: "repeat-guard".into(), message: format!("Stopped: the same {name} call failed the same way {count} times. Say what to do next.") });
            return true;
        }
        false
    }

    fn has_queued(&self) -> bool {
        !self.inbox.lock().expect("inbox").is_empty()
    }

    /// Uses a flag made before the engine, e.g. by the thread that will set it.
    pub fn with_stop_flag(mut self, stop: StopFlag) -> Self {
        self.stop = stop;
        self
    }

    pub fn state(&self) -> SessionState {
        self.state.borrow().clone()
    }

    fn emit(&self, event: Event) {
        let seq = self.seq.get();
        self.seq.set(seq + 1);
        let entry = LogEntry { seq, at_ms: self.parts.platform.clock.now_ms(), event };
        self.state.borrow_mut().apply(&entry.event);
        self.frontend.on_entry(&entry);
        self.unsaved.borrow_mut().push(entry);
    }

    async fn flush(&self) -> Result<(), String> {
        let entries = std::mem::take(&mut *self.unsaved.borrow_mut());
        if entries.is_empty() {
            return Ok(());
        }
        self.parts.store.append(&self.session, &entries).await.map_err(|e| format!("session store: {e}"))
    }

    fn set_task(&self, to: TaskState, reason: Option<String>) {
        let from = self.state.borrow().task;
        if from == to {
            return;
        }
        debug_assert!(from.can_become(to), "illegal task transition {from:?} -> {to:?}");
        if from.can_become(to) {
            self.emit(Event::TaskState { to, reason });
        }
    }

    fn fail(&self, source: &str, message: String, protocol: Option<&Protocol>) -> RunEnd {
        self.emit(Event::Error { source: source.to_string(), message: message.clone() });
        if protocol.is_some() {
            self.set_task(TaskState::Failed, Some(message.clone()));
        }
        RunEnd::Failed(message)
    }

    fn stopped(&self, protocol: Option<&Protocol>) -> RunEnd {
        if protocol.is_some() {
            self.set_task(TaskState::Abandoned, Some("stopped by the user".into()));
        }
        RunEnd::Abandoned
    }

    /// Calls that will not run still get results: the history must stay valid for the model.
    fn refuse_calls(&self, calls: &[ToolCall], why: &str) {
        for call in calls {
            self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
            self.emit(Event::ToolResult { id: call.id.clone(), output: why.to_string(), is_error: true });
        }
    }

    /// Runs every hook on `point`. Returns the first veto.
    async fn hooks(&self, point: HookPoint<'_>) -> Option<(String, String)> {
        let mut veto = None;
        for hook in self.parts.hooks.iter().filter(|h| h.applies(&point)) {
            let outcome = hook.run(&point).await;
            if matches!(outcome, HookOutcome::Continue { .. }) {
                self.go_on.set(true);
            }
            if let (HookOutcome::Blocked { reason }, None) = (&outcome, &veto) {
                veto = Some((hook.name().to_string(), reason.clone()));
            }
            if hook.quiet_when_ok() && outcome == HookOutcome::Ok {
                continue;
            }
            self.emit(Event::HookRan { point: point.kind(), hook: hook.name().to_string(), outcome, detail: None });
        }
        veto
    }

    fn spent(&self) -> Spent {
        let state = self.state.borrow();
        Spent {
            turns: state.turns,
            usage: state.usage,
            cost_micro_usd: state.cost_micro_usd,
            elapsed_ms: self.parts.platform.clock.now_ms().saturating_sub(self.started_ms),
        }
    }

    fn tool_specs(&self) -> Vec<ToolSpec> {
        let mut specs: Vec<ToolSpec> = self.parts.tools.values().map(|t| t.spec()).collect();
        // A small model believes a tool's description over the system prompt.
        if self.state.borrow().plan_mode {
            for spec in &mut specs {
                match spec.name.as_str() {
                    "edit" => spec.description = format!("PLAN MODE: only paths under docs/ can be written until the user approves plan_exit; anything else is refused and nothing is created. {}", spec.description),
                    "shell" => spec.description = format!("PLAN MODE: only one read-only command at a time (ls, cat, grep, git log...); nothing can be run or created until the user approves plan_exit. {}", spec.description),
                    _ => {}
                }
            }
        }
        specs.push(ToolSpec {
            name: plan::ENTER_TOOL.into(),
            description: "Enter plan mode before building an app, a prototype, a feature or a fix, or any change that is not trivial, even if nobody said plan and even if the result was asked for directly. Study and plan there; nothing is built until the user approves.".into(),
            schema: serde_json::json!({"type": "object", "properties": {}}),
        });
        specs.push(ToolSpec {
            name: plan::EXIT_TOOL.into(),
            description: "Show the user the plan and design, after asking what only they can decide and journaling it, and ask to leave plan mode and start the work. Only the user's approval leaves the mode.".into(),
            schema: serde_json::json!({"type": "object", "properties": {"plan": {"type": "string", "description": "The plan in short, with the journal ids of its work items."}}, "required": ["plan"]}),
        });
        specs.push(ToolSpec {
            name: ASK_TOOL.into(),
            description: "Ask the user what only they can decide, and wait: the question is shown, your turn ends, and the answer comes as their next message. Offer options when the answers are known: the user picks one or types their own. One question per call.".into(),
            schema: serde_json::json!({"type": "object", "properties": {"question": {"type": "string"}, "options": {"type": "array", "maxItems": harness_api::question::MAX_OPTIONS, "description": "The likely answers, best first.", "items": {"type": "object", "properties": {"label": {"type": "string"}, "description": {"type": "string", "description": "What choosing this means."}}, "required": ["label"]}}}, "required": ["question"]}),
        });
        specs.push(ToolSpec {
            name: NICK_TOOL.into(),
            description: "Change the name you go by in this session; it replaces ASSISTANT above your messages. Use it when the user names you or asks you to pick a name.".into(),
            schema: serde_json::json!({"type": "object", "properties": {"nick": {"type": "string", "description": "1 to 32 characters, one line."}}, "required": ["nick"]}),
        });
        specs.push(ToolSpec {
            name: DEBUG_MODE_TOOL.into(),
            description: "Switch debug mode. Turn it on for every planned fix, before changing anything, and for every planned feature once it is built and running; turn it off when the result is verified and the evidence is in the journal. A large pipeline, above all a distributed one, is also walked step by step; a small program is checked by running it and by tests.".into(),
            schema: serde_json::json!({"type": "object", "properties": {"on": {"type": "boolean"}}, "required": ["on"]}),
        });
        specs
    }

    /// Names the agent. A bad name changes nothing and says why.
    pub async fn set_agent_name(&self, name: &str, by: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NICK || name.chars().any(char::is_control) {
            return Err(format!("a nick is 1 to {MAX_NICK} characters on one line"));
        }
        if self.state.borrow().agent_name.as_deref() != Some(name) {
            self.emit(Event::AgentName { name: name.to_string(), by: by.to_string() });
            let _ = self.flush().await;
        }
        Ok(())
    }

    /// The user named the agent: a bad name shows as an error, and nothing changes.
    pub async fn set_agent_name_by_user(&self, name: &str) {
        if let Err(message) = self.set_agent_name(name, "user").await {
            self.emit(Event::Error { source: "nick".into(), message });
            let _ = self.flush().await;
        }
    }

    /// Switches step-debug mode (ADR 0013). `by` is who did it, for the log.
    pub async fn set_step_debug(&self, on: bool, by: &str) {
        if self.state.borrow().step_debug == on {
            return;
        }
        self.emit(Event::StepDebug { on, by: by.to_string() });
        self.hooks(if on { HookPoint::StepDebugEnter } else { HookPoint::StepDebugExit }).await;
        let _ = self.flush().await;
    }

    /// Switches plan mode (ADR 0016). `by` is who did it, for the log.
    pub async fn set_plan_mode(&self, on: bool, by: &str) {
        if self.state.borrow().plan_mode == on {
            return;
        }
        self.emit(Event::PlanMode { on, by: by.to_string() });
        self.hooks(if on { HookPoint::PlanEnter } else { HookPoint::PlanExit }).await;
        // A mode set between runs still reaches the log.
        let _ = self.flush().await;
    }

    /// A session made on another provider goes on here: its history is written in the backend's own
    /// form first (IF-18). All but this message's own words, which go as the prompt.
    async fn hand_over(&self) -> Result<(), String> {
        let state = self.state.borrow().clone();
        let Some(format) = self.parts.conversations.clone().filter(|_| state.backend_session.is_none() && state.messages.len() > 1) else { return Ok(()) };
        let mut history = harness_api::provider_ready(&state.messages[state.compacted_upto.min(state.messages.len())..]);
        if let Some(last) = history.last_mut().filter(|m| m.role == harness_api::Role::User) {
            last.content.retain(|c| !matches!(c, Content::Text { .. }));
        }
        history.retain(|m| !m.content.is_empty());
        if history.is_empty() {
            return Ok(());
        }
        let id = format.write(&self.root, &self.session, &history).map_err(|e| e.to_string())?;
        self.emit(Event::BackendSession { id });
        Ok(())
    }

    /// The turns of a message on an agent backend (IF-17): it does the work, and the engine keeps
    /// the log, the stop and the verdict.
    async fn run_backend(&self, backend: &dyn harness_api::AgentBackend, protocol: Option<&Protocol>) -> RunEnd {
        let source = backend.name().to_string();
        if let Err(message) = self.hand_over().await {
            return self.fail(&source, message, protocol);
        }
        loop {
            if let Err(message) = self.flush().await {
                return self.fail("store", message, protocol);
            }
            let state = self.state.borrow().clone();
            let prompt = state.messages.iter().rev().find(|m| m.role == harness_api::Role::User).map_or(String::new(), |m| m.content.iter().filter_map(|c| if let Content::Text { text } = c { Some(text.as_str()) } else { None }).collect::<Vec<_>>().join("\n"));
            let context = self.parts.context.build(&state, self.parts.memory.as_ref()).await;
            let system = context.ok().and_then(|c| c.system);
            let stopped = || self.stop.is_set();
            let request = harness_api::BackendRequest { prompt: &prompt, session: state.backend_session.as_deref(), fork: state.backend_fork, cost_total_micro_usd: state.backend_cost_total, system: system.as_deref(), plan_mode: state.plan_mode, turn: state.turns + 1, stopped: &stopped };
            let mut sink = |event: Event| {
                self.emit(event);
                !self.stop.is_set()
            };
            let end = backend.run(request, &mut sink).await;
            if let Err(message) = self.flush().await {
                return self.fail("store", message, protocol);
            }
            match end {
                Err(e) => return self.fail(&source, e.to_string(), protocol),
                Ok(harness_api::BackendEnd::Stopped) => {
                    self.stop.take();
                    return self.stopped(protocol);
                }
                Ok(harness_api::BackendEnd::Done) => {}
            }
            if self.take_queued().await {
                continue;
            }
            self.hooks(HookPoint::Stop).await;
            let Some(protocol) = protocol.filter(|_| !self.state.borrow().plan_mode) else {
                return RunEnd::AwaitingInput;
            };
            match self.verify(protocol, &prompt).await {
                Ok(true) => return RunEnd::Done,
                Ok(false) => {}
                Err(message) => return self.fail("verifier", message, Some(protocol)),
            }
        }
    }

    /// The two tools the engine itself answers: the mode is its own state.
    async fn plan_tool(&self, call: &ToolCall) {
        self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
        let result = |output: String, is_error: bool| self.emit(Event::ToolResult { id: call.id.clone(), output, is_error });
        if call.name == plan::ENTER_TOOL {
            self.set_plan_mode(true, "agent").await;
            return result("Plan mode is on. Follow its rules; plan_exit asks the user to approve the plan.".into(), false);
        }
        if !self.state.borrow().plan_mode {
            return result("plan mode is not on".into(), true);
        }
        if call.input["plan"].as_str().is_none_or(|p| p.trim().is_empty()) {
            return result("plan is required: the plan to show the user".into(), true);
        }
        // A hook may send it back first, to finish the plan; the user is not asked yet.
        if let Some((_, reason)) = self.hooks(HookPoint::PreToolUse { call }).await {
            return result(reason, true);
        }
        // Always the user's call: nothing is remembered, and an "always" counts this once.
        let answer = self.frontend.ask(call, &[Capability::Command("leave plan mode and start the work".into())]).await;
        if answer == Answer::Deny {
            self.emit(Event::ToolDecision { id: call.id.clone(), allowed: false, by: DecidedBy::User, reason: None });
            return result("The user did not approve the plan. Plan mode stays on: ask what to change.".into(), true);
        }
        self.emit(Event::ToolDecision { id: call.id.clone(), allowed: true, by: DecidedBy::User, reason: None });
        self.set_plan_mode(false, "user").await;
        result("The user approved the plan. Plan mode is off. A fix: call debug_mode with on true now, reproduce it before changing anything, verify after. A feature: build it, run it, then call debug_mode with on true and debug it.".into(), false);
    }

    /// One user message: runs turns until the model stops, then verifies when there is a
    /// protocol. Later messages continue the same session.
    pub async fn run(&self, text: &str, protocol: Option<&Protocol>) -> RunEnd {
        self.run_event(Event::UserMessage { text: text.to_string() }, text, protocol).await
    }

    /// A callback for the agent: the same run, its text marked as external data (INV-12).
    pub async fn run_inbound(&self, label: &str, text: &str) -> RunEnd {
        self.run_event(Event::Inbound { label: label.to_string(), text: text.to_string() }, text, None).await
    }

    async fn run_event(&self, event: Event, text: &str, protocol: Option<&Protocol>) -> RunEnd {
        let end = self.run_inner(event, text, protocol).await;
        if let Err(message) = self.flush().await {
            self.emit(Event::Error { source: "store".into(), message: message.clone() });
            return RunEnd::Failed(message);
        }
        end
    }

    async fn run_inner(&self, event: Event, text: &str, protocol: Option<&Protocol>) -> RunEnd {
        if !self.started.replace(true) {
            self.hooks(HookPoint::SessionStart).await;
        }
        self.failures.borrow_mut().clear();
        if self.state.borrow().task.is_final() {
            return RunEnd::Failed("the task has already ended".into());
        }
        // A stop that came while nothing ran is not for the next message.
        self.stop.take();
        if let Some((hook, reason)) = self.hooks(HookPoint::UserPromptSubmit { text }).await {
            self.emit(Event::Error { source: hook, message: reason.clone() });
            return RunEnd::Failed(reason);
        }
        self.emit(event);
        self.set_task(TaskState::Running, None);
        if let Some(backend) = self.parts.backend.clone() {
            return self.run_backend(backend.as_ref(), protocol).await;
        }
        // From here on the backend's own session lacks what this provider does.
        if self.state.borrow().backend_session.is_some() {
            self.emit(Event::BackendSessionLost);
        }

        loop {
            if let Err(message) = self.flush().await {
                return self.fail("store", message, protocol);
            }
            if self.stop.take() {
                return self.stopped(protocol);
            }
            self.take_queued().await;
            let route = match self.parts.router.route(&self.spent()) {
                Ok(route) => route,
                Err(reason) => return self.fail("router", reason, protocol),
            };
            let (calls, stop) = match self.turn(&route).await {
                Ok(end) => end,
                Err(message) => return self.fail(&route.provider, message, protocol),
            };
            if stop == StopReason::Stopped || self.stop.take() {
                self.stop.take();
                self.refuse_calls(&calls, "stopped by the user");
                return self.stopped(protocol);
            }
            if stop == StopReason::MaxTokens && !calls.is_empty() {
                // The arguments may be cut off: never run them. The model hears why.
                self.refuse_calls(&calls, "the turn hit the token limit and this call may be incomplete; it did not run");
                continue;
            }
            if !calls.is_empty() {
                for call in &calls {
                    self.tool_call(call).await;
                }
                if self.stuck(&calls) {
                    let _ = self.flush().await;
                    return RunEnd::AwaitingInput;
                }
                if self.asked.take() && !self.has_queued() {
                    let _ = self.flush().await;
                    return RunEnd::AwaitingInput;
                }
                continue;
            }
            // The user wrote meanwhile: the run goes on with it.
            if self.has_queued() {
                continue;
            }
            self.hooks(HookPoint::Stop).await;
            if self.go_on.take() {
                continue;
            }
            // Running a protocol is not planning: a task does not finish in plan mode.
            let Some(protocol) = protocol.filter(|_| !self.state.borrow().plan_mode) else {
                return RunEnd::AwaitingInput;
            };
            match self.verify(protocol, text).await {
                Ok(true) => return RunEnd::Done,
                Ok(false) => {} // the evidence went into the context; the model takes it up
                Err(message) => return self.fail("verifier", message, Some(protocol)),
            }
        }
    }

    async fn turn(&self, route: &Route) -> Result<(Vec<ToolCall>, StopReason), String> {
        let provider = self.parts.providers.get(&route.provider).ok_or_else(|| format!("the router chose an unknown provider: {}", route.provider))?;
        let context = {
            let mut state = self.state.borrow().clone();
            state.enter_model(&route.provider, &route.model);
            self.parts.context.build(&state, self.parts.memory.as_ref()).await.map_err(|e| format!("context: {e}"))?
        };
        if let Some(cut) = context.compacted {
            self.emit(Event::Compacted { upto: cut.upto, tokens_before: cut.tokens_before, tokens_after: cut.tokens_after });
            self.hooks(HookPoint::PostCompact).await;
        }
        let turn = self.state.borrow().turns + 1;
        self.emit(Event::TurnStarted { turn, provider: route.provider.clone(), model: route.model.clone() });
        let request = ModelRequest {
            model: route.model.clone(),
            system: context.system,
            messages: context.messages,
            tools: self.tool_specs(),
            max_tokens: route.max_tokens,
        };
        let calls = RefCell::new(Vec::new());
        let mut sink = |event: ModelEvent| {
            match event {
                ModelEvent::Text(text) => self.emit(Event::TextDelta { text }),
                ModelEvent::Thinking(text) => self.emit(Event::ThinkingDelta { text }),
                ModelEvent::Opaque(block) => self.emit(Event::OpaqueBlock { block }),
                ModelEvent::ToolCall(call) => {
                    self.emit(Event::ModelToolCall { call: call.clone() });
                    calls.borrow_mut().push(call);
                }
            }
            !self.stop.is_set()
        };
        let end = provider.turn(&request, &mut sink).await.map_err(|e| e.to_string())?;
        self.emit(Event::TurnFinished { turn, stop: end.stop.clone(), usage: end.usage, cost_micro_usd: end.cost_micro_usd });
        if let StopReason::Other(reason) = &end.stop {
            return Err(format!("the model stopped: {reason}"));
        }
        Ok((calls.into_inner(), end.stop))
    }

    /// One tool call through hooks and `Policy` (INV-3). Whatever happens, the model gets a
    /// result for it.
    async fn tool_call(&self, call: &ToolCall) {
        if call.name == plan::ENTER_TOOL || call.name == plan::EXIT_TOOL {
            return self.plan_tool(call).await;
        }
        if call.name == ASK_TOOL {
            self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
            let (output, is_error) = match harness_api::question::Question::parse(&call.input) {
                Ok(question) => {
                    self.asked.set(true);
                    let options: String = question.options.iter().enumerate().map(|(i, o)| format!("\n{}. {}", i + 1, o.label)).collect();
                    (format!("Asked the user: {}{options}\nTheir answer comes as their next message.", question.text), false)
                }
                Err(e) => (e, true),
            };
            self.emit(Event::ToolResult { id: call.id.clone(), output, is_error });
            return;
        }
        if call.name == NICK_TOOL {
            self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
            let answer = match call.input["nick"].as_str() {
                Some(nick) => self.set_agent_name(nick, "agent").await.map(|()| format!("You go by {} now.", nick.trim())),
                None => Err("nick is required".to_string()),
            };
            let (output, is_error) = answer.map_or_else(|e| (e, true), |ok| (ok, false));
            self.emit(Event::ToolResult { id: call.id.clone(), output, is_error });
            return;
        }
        if call.name == DEBUG_MODE_TOOL {
            self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
            let (output, is_error) = match call.input["on"].as_bool() {
                Some(true) if self.state.borrow().plan_mode => ("You are in plan mode: debugging comes after the user approves the plan. Finish the plan, then call plan_exit.".to_string(), true),
                Some(on) => {
                    self.set_step_debug(on, "agent").await;
                    (format!("Step-debug mode is {}.", if on { "on" } else { "off" }), false)
                }
                None => ("on is required: true or false".to_string(), true),
            };
            self.emit(Event::ToolResult { id: call.id.clone(), output, is_error });
            return;
        }
        let tool = self.parts.tools.get(&call.name).cloned();
        let capabilities = match tool.as_ref().map(|t| t.capabilities(&call.input)) {
            Some(Ok(capabilities)) => capabilities,
            Some(Err(reason)) => {
                self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
                self.emit(Event::ToolResult { id: call.id.clone(), output: format!("invalid arguments: {reason}"), is_error: true });
                return;
            }
            None => {
                self.emit(Event::ToolRequested { call: call.clone(), capabilities: vec![] });
                self.emit(Event::ToolResult { id: call.id.clone(), output: format!("no tool named {}", call.name), is_error: true });
                return;
            }
        };
        let tool = tool.expect("checked above");
        self.emit(Event::ToolRequested { call: call.clone(), capabilities: capabilities.clone() });

        if self.state.borrow().plan_mode
            && let Some(reason) = plan::refusal(&capabilities, &self.root)
        {
            self.emit(Event::ToolDecision { id: call.id.clone(), allowed: false, by: DecidedBy::Mode, reason: Some(reason.clone()) });
            self.emit(Event::ToolResult { id: call.id.clone(), output: reason, is_error: true });
            return;
        }
        if let Some((hook, reason)) = self.hooks(HookPoint::PreToolUse { call }).await {
            self.emit(Event::ToolDecision { id: call.id.clone(), allowed: false, by: DecidedBy::Hook, reason: Some(format!("{hook}: {reason}")) });
            self.emit(Event::ToolResult { id: call.id.clone(), output: format!("blocked by the hook {hook}: {reason}"), is_error: true });
            return;
        }
        if let Err(reason) = self.approve(call, &capabilities).await {
            self.emit(Event::ToolResult { id: call.id.clone(), output: format!("denied: {reason}"), is_error: true });
            return;
        }
        let cx = ToolContext { platform: self.parts.platform.clone(), granted: capabilities, root: self.root.clone() };
        let output: ToolOutput = tool.execute(&call.input, &cx).await;
        self.emit(Event::ToolResult { id: call.id.clone(), output: output.text.clone(), is_error: output.is_error });
        self.hooks(HookPoint::PostToolUse { call, output: &output }).await;
    }

    async fn approve(&self, call: &ToolCall, capabilities: &[Capability]) -> Result<(), String> {
        let decided = |allowed: bool, by: DecidedBy, reason: Option<String>| self.emit(Event::ToolDecision { id: call.id.clone(), allowed, by, reason });
        match self.parts.policy.decide(call, capabilities) {
            Decision::Allow => {
                decided(true, DecidedBy::Policy, None);
                Ok(())
            }
            Decision::Deny(reason) => {
                decided(false, DecidedBy::Policy, Some(reason.clone()));
                Err(reason)
            }
            Decision::Ask => {
                let answer = self.frontend.ask(call, capabilities).await;
                self.parts.policy.remember(call, capabilities, answer);
                if answer == Answer::Deny {
                    decided(false, DecidedBy::User, None);
                    Err("the user said no".into())
                } else {
                    decided(true, DecidedBy::User, None);
                    Ok(())
                }
            }
        }
    }

    /// Returns whether the task is done. A failed check goes back to running, with the evidence
    /// in the conversation.
    /// INV-11: the specifications no step of this protocol checks, as a line of evidence.
    async fn unchecked_specs(&self, protocol: &Protocol) -> String {
        let binding = self.parts.memory.journal_binding(UNCHECKED_SCAN).await.unwrap_or_default();
        let ids: Vec<String> = binding
            .iter()
            .filter(|e| e.flags.iter().any(|f| f == "specification") && !protocol.specs.contains(&e.id))
            .map(|e| format!("#{}", e.id))
            .collect();
        if ids.is_empty() { String::new() } else { format!("\nSpecifications with no check: {}", ids.join(", ")) }
    }

    async fn verify(&self, protocol: &Protocol, task: &str) -> Result<bool, String> {
        self.set_task(TaskState::Verifying, None);
        let state = self.state.borrow().clone();
        let mut result = self.parts.verifier.verify(protocol, &state).await.map_err(|e| e.to_string())?;
        result.evidence.push_str(&self.unchecked_specs(protocol).await);
        self.emit(Event::Verification { pass: result.pass, evidence: result.evidence.clone() });
        // IF-10: the evidence goes into memory. A store that cannot take it does not change the verdict.
        let entry = JournalEntry {
            body: format!("Verification {} for the task: {task}\n{}", if result.pass { "passed" } else { "failed" }, result.evidence),
            category: "verification".into(),
            tags: vec!["verification".into()],
            flags: if result.pass { vec!["verified".into()] } else { vec![] },
            salience: Some(if result.pass { "medium" } else { "low" }.into()),
            feature: None,
        };
        if let Err(e) = self.parts.memory.journal_add(entry).await {
            self.emit(Event::Error { source: "memory".into(), message: format!("the evidence was not recorded: {e}") });
        }
        if result.pass {
            self.set_task(TaskState::Done, None);
            return Ok(true);
        }
        self.set_task(TaskState::Running, Some("verification failed".into()));
        self.emit(Event::UserMessage { text: format!("Verification failed:\n{}", result.evidence) });
        Ok(false)
    }
}
