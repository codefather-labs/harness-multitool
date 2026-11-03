//! A test double for every interface (roadmap M1, INV-2): a scripted provider and simple,
//! in-memory versions of the rest. `multitool trace` and the tests run the real loop on them.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use harness_api::event::HookOutcome;
use harness_api::platform::{Clock, Fs, Net, NetRequest, NetResponse, Platform, Process, ProcessOutput, Storage, Terminal};
use harness_api::*;
use serde_json::{Value, json};

fn ready<'a, T: 'a>(value: T) -> BoxFuture<'a, T> {
    Box::pin(std::future::ready(value))
}

/// One scripted model turn.
#[derive(Debug, Clone, Default)]
pub struct ScriptedTurn {
    pub text: Vec<String>,
    pub calls: Vec<ToolCall>,
    /// When set, the turn fails with this message.
    pub error: Option<String>,
    /// The turn ends cut off by the token limit.
    pub max_tokens: bool,
}

impl ScriptedTurn {
    pub fn say(text: &str) -> Self {
        // Streamed word by word, as a real provider would.
        let text = text.split_inclusive(' ').map(str::to_string).collect();
        ScriptedTurn { text, ..Default::default() }
    }

    pub fn call(id: &str, name: &str, input: Value) -> Self {
        ScriptedTurn { calls: vec![ToolCall { id: id.into(), name: name.into(), input }], ..Default::default() }
    }

    pub fn fail(message: &str) -> Self {
        ScriptedTurn { error: Some(message.into()), ..Default::default() }
    }
}

/// IF-1. Plays its turns in order; an exhausted script ends the turn with no text.
pub struct ScriptedProvider {
    turns: RefCell<VecDeque<ScriptedTurn>>,
    /// The requests it received, for tests to inspect.
    pub requests: RefCell<Vec<ModelRequest>>,
}

impl ScriptedProvider {
    pub fn new(turns: Vec<ScriptedTurn>) -> Rc<Self> {
        Rc::new(ScriptedProvider { turns: RefCell::new(turns.into()), requests: RefCell::default() })
    }
}

impl Provider for ScriptedProvider {
    fn name(&self) -> &str {
        "scripted"
    }

    fn models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo { id: "scripted-1".into(), context_window: 200_000, supports_tools: true }]
    }

    fn turn<'a>(&'a self, request: &'a ModelRequest, sink: &'a mut dyn FnMut(ModelEvent) -> bool) -> BoxFuture<'a, PluginResult<TurnEnd>> {
        self.requests.borrow_mut().push(request.clone());
        let turn = self.turns.borrow_mut().pop_front().unwrap_or_default();
        Box::pin(async move {
            if let Some(message) = turn.error {
                return Err(PluginError(message));
            }
            let usage = Usage { input: 100, output: 20, cache_read: 0, cache_write: 0 };
            for text in turn.text {
                if !sink(ModelEvent::Text(text)) {
                    return Ok(TurnEnd { stop: StopReason::Stopped, usage, cost_micro_usd: Some(120) });
                }
            }
            let stop = if turn.max_tokens {
                StopReason::MaxTokens
            } else if turn.calls.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            };
            for call in turn.calls {
                sink(ModelEvent::ToolCall(call));
            }
            Ok(TurnEnd { stop, usage, cost_micro_usd: Some(120) })
        })
    }
}

/// IF-2.
pub struct FixedAuth(pub Secret);

impl Auth for FixedAuth {
    fn credentials(&self, _provider: &str) -> BoxFuture<'_, PluginResult<Secret>> {
        ready(Ok(self.0.clone()))
    }
}

/// IF-3. One model, a budget in turns.
pub struct OneModelRouter {
    pub provider: String,
    pub max_turns: u32,
}

impl Router for OneModelRouter {
    fn route(&self, spent: &Spent) -> Result<Route, String> {
        if spent.turns >= self.max_turns {
            return Err(format!("budget exhausted: {} turns", self.max_turns));
        }
        Ok(Route { provider: self.provider.clone(), model: "scripted-1".into(), max_tokens: 1024 })
    }
}

/// IF-4. Echoes its `text` argument; needs what it is told to need.
pub struct EchoTool {
    pub name: String,
    pub needs: Vec<Capability>,
}

impl Tool for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.clone(),
            description: "Echoes its text.".into(),
            schema: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
        }
    }

    fn capabilities(&self, _input: &Value) -> Result<Vec<Capability>, String> {
        Ok(self.needs.clone())
    }

    fn execute<'a>(&'a self, input: &'a Value, _cx: &'a ToolContext) -> BoxFuture<'a, ToolOutput> {
        ready(ToolOutput::ok(input["text"].as_str().unwrap_or_default()))
    }
}

/// IF-5. Decides the same way every time, remembers the answers it got.
pub struct FixedPolicy {
    pub decision: Decision,
    pub answers: RefCell<Vec<Answer>>,
}

impl FixedPolicy {
    pub fn new(decision: Decision) -> Self {
        FixedPolicy { decision, answers: RefCell::default() }
    }
}

impl Policy for FixedPolicy {
    fn decide(&self, _call: &ToolCall, _capabilities: &[Capability]) -> Decision {
        self.decision.clone()
    }

    fn remember(&self, _call: &ToolCall, _capabilities: &[Capability], answer: Answer) {
        self.answers.borrow_mut().push(answer);
    }
}

/// IF-6. Gives the same outcome on the point it watches, `Ok` elsewhere.
pub struct FixedHook {
    pub name: String,
    pub on: harness_api::event::HookPointKind,
    pub outcome: HookOutcome,
}

impl Hook for FixedHook {
    fn name(&self) -> &str {
        &self.name
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        ready(if point.kind() == self.on { self.outcome.clone() } else { HookOutcome::Ok })
    }
}

/// IF-7. Entries and maps in memory.
#[derive(Default)]
pub struct InMemoryJournal {
    pub entries: RefCell<Vec<JournalRecord>>,
    pub maps: RefCell<Vec<MapData>>,
}

impl Memory for InMemoryJournal {
    fn journal_add(&self, entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
        let id = self.entries.borrow().len() as u64 + 1;
        let record = JournalRecord { blocked_by: Vec::new(), blocker_dropped: Vec::new(), proofs: entry.proofs.clone(), open: entry.flags.iter().any(|f| f == "todo"), id, created: String::new(), category: entry.category, tags: entry.tags, body: entry.body, flags: entry.flags };
        self.entries.borrow_mut().push(record.clone());
        ready(Ok(record))
    }

    fn journal_search(&self, query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        ready(Ok(self.entries.borrow().iter().filter(|e| e.body.contains(query)).cloned().collect()))
    }

    fn journal_get(&self, id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
        ready(Ok(self.entries.borrow().iter().find(|e| e.id == id).cloned()))
    }

    fn journal_close(&self, _id: u64, _resolution: &str) -> BoxFuture<'_, PluginResult<()>> {
        ready(Ok(()))
    }

    fn journal_binding(&self, limit: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        let binding = |e: &&JournalRecord| e.flags.iter().any(|f| matches!(f.as_str(), "specification" | "constraint" | "invariant" | "principle")) && !e.flags.iter().any(|f| f == "deprecated");
        ready(Ok(self.entries.borrow().iter().rev().filter(binding).take(limit).cloned().collect()))
    }

    fn journal_list(&self, filter: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        let wanted = |e: &&JournalRecord| filter.category.as_ref().is_none_or(|c| *c == e.category) && (filter.flags.is_empty() || e.flags.iter().any(|f| filter.flags.contains(f)));
        ready(Ok(self.entries.borrow().iter().rev().filter(wanted).take(filter.limit).cloned().collect()))
    }

    fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>> {
        ready(Ok(self.maps.borrow().iter().map(|m| (m.id, m.title.clone())).collect()))
    }

    fn map_new(&self, kind: MapKind, title: &str, direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
        let id = self.maps.borrow().len() as u64 + 1;
        let root = MapNode { id: 1, parent: None, kind: "topic".into(), text: title.into(), state: "open".into(), refs: vec![] };
        let map = MapData { id, title: title.into(), kind, direction: direction.unwrap_or("right").into(), root: 1, nodes: vec![root], edges: vec![] };
        self.maps.borrow_mut().push(map.clone());
        ready(Ok(map))
    }

    fn map_add(&self, map: u64, node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
        let mut maps = self.maps.borrow_mut();
        let result = maps.iter_mut().find(|m| m.id == map).map(|m| {
            let id = m.nodes.len() as u64 + 1;
            m.nodes.push(MapNode { id, parent: node.parent.or(Some(m.root)), kind: node.kind.clone(), text: node.text.clone(), state: "open".into(), refs: node.refs.clone() });
            if let Some(from) = node.after {
                m.edges.push(MapEdge { from, to: id, label: node.label.clone(), refs: vec![] });
            }
            m.clone()
        });
        ready(result.ok_or_else(|| PluginError::new(format!("no map #{map}"))))
    }

    fn map_read(&self, map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
        ready(Ok(self.maps.borrow().iter().find(|m| m.id == map).cloned()))
    }
}

/// IF-8. The whole conversation, hook context in the system prompt.
pub struct WholeConversation;

impl ContextStrategy for WholeConversation {
    fn build<'a>(&'a self, state: &'a SessionState, _memory: &'a dyn Memory) -> BoxFuture<'a, PluginResult<TurnContext>> {
        let system = (!state.hook_context.is_empty()).then(|| state.hook_context.join("\n"));
        ready(Ok(TurnContext { system, messages: state.messages.clone(), compacted: None }))
    }
}

/// IF-9.
#[derive(Default)]
pub struct InMemoryStore {
    pub sessions: RefCell<BTreeMap<String, Vec<LogEntry>>>,
}

impl SessionStore for InMemoryStore {
    fn append<'a>(&'a self, session: &'a str, entries: &'a [LogEntry]) -> BoxFuture<'a, PluginResult<()>> {
        self.sessions.borrow_mut().entry(session.to_string()).or_default().extend_from_slice(entries);
        ready(Ok(()))
    }

    fn load<'a>(&'a self, session: &'a str) -> BoxFuture<'a, PluginResult<Vec<LogEntry>>> {
        ready(Ok(self.sessions.borrow().get(session).cloned().unwrap_or_default()))
    }

    fn list(&self) -> BoxFuture<'_, PluginResult<Vec<String>>> {
        ready(Ok(self.sessions.borrow().keys().cloned().collect()))
    }
}

/// IF-10. Plays its verdicts in order; an exhausted script fails.
pub struct ScriptedVerifier {
    verdicts: RefCell<VecDeque<Verification>>,
}

impl ScriptedVerifier {
    pub fn new(verdicts: Vec<(bool, &str)>) -> Self {
        let verdicts = verdicts.into_iter().map(|(pass, evidence)| Verification { pass, evidence: evidence.into() }).collect();
        ScriptedVerifier { verdicts: RefCell::new(verdicts) }
    }
}

impl Verifier for ScriptedVerifier {
    fn verify<'a>(&'a self, _protocol: &'a Protocol, _state: &'a SessionState) -> BoxFuture<'a, PluginResult<Verification>> {
        let verdict = self.verdicts.borrow_mut().pop_front();
        ready(Ok(verdict.unwrap_or(Verification { pass: false, evidence: "no verdict scripted".into() })))
    }
}

/// IF-11. Records what it sees and answers prompts from a script (deny when exhausted).
#[derive(Default)]
pub struct RecordingFrontend {
    pub entries: RefCell<Vec<LogEntry>>,
    pub answers: RefCell<VecDeque<Answer>>,
    pub asked: Cell<u32>,
}

impl Frontend for RecordingFrontend {
    fn on_entry(&self, entry: &LogEntry) {
        self.entries.borrow_mut().push(entry.clone());
    }

    fn ask<'a>(&'a self, _call: &'a ToolCall, _capabilities: &'a [Capability]) -> BoxFuture<'a, Answer> {
        self.asked.set(self.asked.get() + 1);
        ready(self.answers.borrow_mut().pop_front().unwrap_or(Answer::Deny))
    }
}

/// IF-12.
#[derive(Default)]
pub struct RecordingTransport {
    pub sent: RefCell<Vec<(String, String)>>,
}

impl Transport for RecordingTransport {
    fn send<'a>(&'a self, to: &'a str, message: &'a str) -> BoxFuture<'a, PluginResult<()>> {
        self.sent.borrow_mut().push((to.to_string(), message.to_string()));
        ready(Ok(()))
    }
}

/// IF-13. Knows one type; a repeated uuid is a duplicate (INV-14).
pub struct OneTypeTrigger {
    pub kind: String,
    pub seen: RefCell<Vec<String>>,
}

impl Trigger for OneTypeTrigger {
    fn route<'a>(&'a self, callback: &'a Callback) -> BoxFuture<'a, PluginResult<Routed>> {
        let routed = if self.seen.borrow().contains(&callback.uuid) {
            Routed::Duplicate
        } else if callback.kind != self.kind {
            Routed::NoHandler
        } else {
            self.seen.borrow_mut().push(callback.uuid.clone());
            Routed::Handled { handler: "double".into() }
        };
        ready(Ok(routed))
    }
}

/// IF-14.
pub struct FixedSkill;

impl Skill for FixedSkill {
    fn name(&self) -> &str {
        "double"
    }

    fn instructions(&self) -> &str {
        "Be a test double."
    }

    fn tools(&self) -> Vec<String> {
        vec![]
    }
}

/// IF-15. True once it has seen `at` signals.
pub struct CountCondition {
    pub at: usize,
}

impl Condition for CountCondition {
    fn name(&self) -> &str {
        "count"
    }

    fn check(&self, signals: &[Callback]) -> bool {
        signals.len() >= self.at
    }

    fn policies(&self) -> Vec<String> {
        vec![]
    }
}

/// IF-16.
#[derive(Default)]
pub struct RecordingHandler {
    pub handled: RefCell<Vec<Callback>>,
}

impl CallbackHandler for RecordingHandler {
    fn name(&self) -> &str {
        "recording"
    }

    fn types(&self) -> Vec<String> {
        vec!["notification".into()]
    }

    fn handle<'a>(&'a self, callback: &'a Callback) -> BoxFuture<'a, PluginResult<HandlerEffect>> {
        self.handled.borrow_mut().push(callback.clone());
        ready(Ok(HandlerEffect::Quiet))
    }
}

/// IF-23. Advances by one millisecond on every reading, so logs are deterministic.
#[derive(Default)]
pub struct StepClock(Cell<u64>);

impl Clock for StepClock {
    fn now_ms(&self) -> u64 {
        self.0.set(self.0.get() + 1);
        self.0.get()
    }

    fn sleep(&self, ms: u64) -> BoxFuture<'_, ()> {
        self.0.set(self.0.get() + ms);
        ready(())
    }
}

/// IF-20, IF-24: files and keys in memory.
#[derive(Default)]
pub struct MemFs(RefCell<BTreeMap<String, Vec<u8>>>);

impl Fs for MemFs {
    fn read(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<u8>>> {
        ready(self.0.borrow().get(path).cloned().ok_or_else(|| PluginError::new(format!("no such file: {path}"))))
    }

    fn write(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        self.0.borrow_mut().insert(path.to_string(), bytes.to_vec());
        ready(Ok(()))
    }

    fn append(&self, path: &str, bytes: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        self.0.borrow_mut().entry(path.to_string()).or_default().extend_from_slice(bytes);
        ready(Ok(()))
    }

    fn list(&self, path: &str) -> BoxFuture<'_, PluginResult<Vec<String>>> {
        ready(Ok(self.0.borrow().keys().filter(|k| k.starts_with(path)).cloned().collect()))
    }
}

impl Storage for MemFs {
    fn get(&self, key: &str) -> BoxFuture<'_, PluginResult<Option<Vec<u8>>>> {
        ready(Ok(self.0.borrow().get(key).cloned()))
    }

    fn put(&self, key: &str, value: &[u8]) -> BoxFuture<'_, PluginResult<()>> {
        self.0.borrow_mut().insert(key.to_string(), value.to_vec());
        ready(Ok(()))
    }
}

/// IF-21.
pub struct NoProcess;

impl Process for NoProcess {
    fn run(&self, _command: &str, _cwd: Option<&str>, _env: &[(String, String)]) -> BoxFuture<'_, PluginResult<ProcessOutput>> {
        ready(Err(PluginError::new("no processes in the test platform")))
    }
}

/// IF-22. Refuses everything: a double never reaches a network.
pub struct NoNet;

impl Net for NoNet {
    fn send(&self, request: NetRequest) -> BoxFuture<'_, PluginResult<NetResponse>> {
        ready(Err(PluginError::new(format!("no network in the test platform: {}", request.url))))
    }
}

/// IF-25.
pub struct FixedTerminal;

impl Terminal for FixedTerminal {
    fn size(&self) -> (u16, u16) {
        (80, 24)
    }
}

/// A platform with no outside world.
pub fn platform() -> Platform {
    Platform {
        fs: Rc::new(MemFs::default()),
        process: Some(Rc::new(NoProcess)),
        net: Rc::new(NoNet),
        clock: Rc::new(StepClock::default()),
        storage: Rc::new(MemFs::default()),
    }
}
