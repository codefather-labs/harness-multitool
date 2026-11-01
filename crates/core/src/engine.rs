//! IF-0.1, the agent loop, and IF-0.2, the bus: every step is an event that updates the state,
//! reaches the frontend and goes to the session log (INV-5).
//!
//! A model that stops ends a *task* only through the verifier (INV-4). A conversation has no
//! protocol, so a stop there waits for the user: never `done`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use harness_api::event::{DecidedBy, HookOutcome};
use harness_api::{
    Answer, Capability, Content, Decision, Event, Frontend, HookPoint, LogEntry, ModelEvent, ModelRequest, Protocol, Route, SessionState, Spent, StopReason, TaskState, ToolCall,
    ToolContext, ToolOutput, ToolSpec,
};

use crate::registry::Parts;

/// Asks the loop to stop at its next step. Safe to set from another thread.
#[derive(Debug, Clone, Default)]
pub struct StopFlag(Arc<AtomicBool>);

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
            if let (HookOutcome::Blocked { reason }, None) = (&outcome, &veto) {
                veto = Some((hook.name().to_string(), reason.clone()));
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
        self.parts.tools.values().map(|t| t.spec()).collect()
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

        loop {
            if let Err(message) = self.flush().await {
                return self.fail("store", message, protocol);
            }
            if self.stop.take() {
                return self.stopped(protocol);
            }
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
                continue;
            }
            self.hooks(HookPoint::Stop).await;
            let Some(protocol) = protocol else {
                return RunEnd::AwaitingInput;
            };
            match self.verify(protocol).await {
                Ok(true) => return RunEnd::Done,
                Ok(false) => {} // the evidence went into the context; the model takes it up
                Err(message) => return self.fail("verifier", message, Some(protocol)),
            }
        }
    }

    async fn turn(&self, route: &Route) -> Result<(Vec<ToolCall>, StopReason), String> {
        let provider = self.parts.providers.get(&route.provider).ok_or_else(|| format!("the router chose an unknown provider: {}", route.provider))?;
        let context = {
            let state = self.state.borrow().clone();
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
    async fn verify(&self, protocol: &Protocol) -> Result<bool, String> {
        self.set_task(TaskState::Verifying, None);
        let state = self.state.borrow().clone();
        let result = self.parts.verifier.verify(protocol, &state).await.map_err(|e| e.to_string())?;
        self.emit(Event::Verification { pass: result.pass, evidence: result.evidence.clone() });
        if result.pass {
            self.set_task(TaskState::Done, None);
            return Ok(true);
        }
        self.set_task(TaskState::Running, Some("verification failed".into()));
        self.emit(Event::UserMessage { text: format!("Verification failed:\n{}", result.evidence) });
        Ok(false)
    }
}
