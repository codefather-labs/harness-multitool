//! Named runs of the real loop on test doubles. A scenario names only doubles in its
//! `Selection`, so it also proves INV-2: no default plugin is linked. `multitool trace <name>`
//! prints a scenario's log line by line, the debug view of the loop.

use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

use harness_api::event::{HookOutcome, HookPointKind};
use harness_api::*;
use harness_core::{Engine, Parts, Registry, RunEnd, Selection, StopFlag};
use harness_doubles::*;
use serde_json::json;

/// What a scenario sets up.
#[derive(Default)]
pub struct Setup {
    pub turns: Vec<ScriptedTurn>,
    pub prompts: Vec<&'static str>,
    pub decision: Option<Decision>,
    pub answers: Vec<Answer>,
    pub hook: Option<(HookPointKind, HookOutcome)>,
    pub protocol: bool,
    pub verdicts: Vec<(bool, &'static str)>,
    pub max_turns: Option<u32>,
    /// Stop after this many text chunks of the first turn; 0 stops before the run.
    pub stop_at_turn: Option<usize>,
}

/// What a scenario produced.
pub struct Trace {
    pub entries: Vec<LogEntry>,
    pub stored: Vec<LogEntry>,
    pub ends: Vec<RunEnd>,
    pub state: SessionState,
    pub asked: u32,
    pub requests: Vec<ModelRequest>,
}

pub const NAMES: &[&str] = &[
    "plain",
    "tool-allow",
    "tool-deny",
    "tool-ask-yes",
    "tool-ask-no",
    "unknown-tool",
    "hook-veto",
    "hook-context",
    "prompt-veto",
    "verify-pass",
    "verify-fail-then-pass",
    "budget",
    "stop",
    "stop-task",
    "stop-mid-turn",
    "max-tokens-tool",
    "provider-error",
    "provider-error-task",
    "two-prompts",
];

fn echo(id: &str) -> ScriptedTurn {
    ScriptedTurn::call(id, "echo", json!({"text": "hello from the tool"}))
}

pub fn setup(name: &str) -> Option<Setup> {
    let done = || ScriptedTurn::say("All done.");
    let s = Setup { prompts: vec!["do the thing"], ..Default::default() };
    Some(match name {
        "plain" => Setup { turns: vec![ScriptedTurn::say("Hello there, human.")], ..s },
        "tool-allow" => Setup { turns: vec![echo("c1"), done()], ..s },
        "tool-deny" => Setup { turns: vec![echo("c1"), done()], decision: Some(Decision::Deny("not in this project".into())), ..s },
        "tool-ask-yes" => Setup { turns: vec![echo("c1"), done()], decision: Some(Decision::Ask), answers: vec![Answer::AllowOnce], ..s },
        "tool-ask-no" => Setup { turns: vec![echo("c1"), done()], decision: Some(Decision::Ask), answers: vec![Answer::Deny], ..s },
        "unknown-tool" => Setup { turns: vec![ScriptedTurn::call("c1", "no-such-tool", json!({})), done()], ..s },
        "hook-veto" => Setup {
            turns: vec![echo("c1"), done()],
            hook: Some((HookPointKind::PreToolUse, HookOutcome::Blocked { reason: "no tests were run".into() })),
            ..s
        },
        "hook-context" => Setup {
            turns: vec![done()],
            hook: Some((HookPointKind::SessionStart, HookOutcome::Context { lines: vec!["Journal your decisions.".into()] })),
            ..s
        },
        "prompt-veto" => Setup {
            hook: Some((HookPointKind::UserPromptSubmit, HookOutcome::Blocked { reason: "not today".into() })),
            ..s
        },
        "verify-pass" => Setup { turns: vec![done()], protocol: true, verdicts: vec![(true, "cargo test: 3 passed")], ..s },
        "verify-fail-then-pass" => Setup {
            turns: vec![done(), ScriptedTurn::say("Fixed it.")],
            protocol: true,
            verdicts: vec![(false, "cargo test: 1 failed"), (true, "cargo test: 3 passed")],
            ..s
        },
        "budget" => Setup { turns: vec![echo("c1"), echo("c2"), echo("c3")], max_turns: Some(2), ..s },
        "stop" => Setup { turns: vec![done(), ScriptedTurn::say("Still here.")], prompts: vec!["one", "two"], stop_at_turn: Some(0), ..s },
        "stop-task" => Setup { turns: vec![done()], protocol: true, stop_at_turn: Some(0), ..s },
        "stop-mid-turn" => Setup { turns: vec![ScriptedTurn::say("one two three four five")], stop_at_turn: Some(1), ..s },
        "max-tokens-tool" => Setup {
            turns: vec![ScriptedTurn { max_tokens: true, ..echo("c1") }, done()],
            ..s
        },
        "provider-error" => Setup { turns: vec![ScriptedTurn::fail("overloaded"), ScriptedTurn::say("Back again.")], prompts: vec!["one", "two"], ..s },
        "provider-error-task" => Setup { turns: vec![ScriptedTurn::fail("overloaded")], protocol: true, ..s },
        "two-prompts" => Setup {
            turns: vec![ScriptedTurn::say("First answer."), ScriptedTurn::say("Second answer.")],
            prompts: vec!["first", "second"],
            ..s
        },
        _ => return None,
    })
}

/// A selection naming only doubles.
pub fn selection() -> Selection {
    Selection {
        auth: "double".into(),
        providers: vec!["scripted".into()],
        router: "double".into(),
        tools: vec!["echo".into()],
        policy: "double".into(),
        hooks: vec!["double".into()],
        memory: "double".into(),
        context: "double".into(),
        store: "double".into(),
        verifier: "double".into(),
    }
}

/// The parts a scenario runs on, for the live UI (`multitool preview --script`).
pub fn parts(setup: Setup) -> Parts {
    let provider = ScriptedProvider::new(setup.turns);
    registry(provider, Rc::new(InMemoryStore::default()), setup.decision, setup.max_turns, setup.hook, setup.verdicts).resolve(&selection(), platform()).expect("the scenario's selection resolves")
}

fn registry(provider: Rc<ScriptedProvider>, store: Rc<InMemoryStore>, decision: Option<Decision>, max_turns: Option<u32>, hook: Option<(HookPointKind, HookOutcome)>, verdicts: Vec<(bool, &'static str)>) -> Registry {
    let mut registry = Registry::new();
    let decision = decision.unwrap_or(Decision::Allow);
    let max_turns = max_turns.unwrap_or(50);
    registry
        .auth("double", |_| Ok(Rc::new(FixedAuth(Secret::new("canary-secret")))))
        .provider("scripted", move |_| Ok(provider.clone()))
        .router("double", move |_| Ok(Rc::new(OneModelRouter { provider: "scripted".into(), max_turns })))
        .tool("echo", |_| Ok(Rc::new(EchoTool { name: "echo".into(), needs: vec![Capability::Command("echo".into())] })))
        .policy("double", move |_| Ok(Rc::new(FixedPolicy::new(decision.clone()))))
        .hook("double", move |_| {
            let (on, outcome) = hook.clone().unwrap_or((HookPointKind::Stop, HookOutcome::Ok));
            Ok(Rc::new(FixedHook { name: "fixture.sh".into(), on, outcome }))
        })
        .memory("double", |_| Ok(Rc::new(InMemoryJournal::default())))
        .context("double", |_| Ok(Rc::new(WholeConversation)))
        .store("double", move |_| Ok(store.clone()))
        .verifier("double", move |_| Ok(Rc::new(ScriptedVerifier::new(verdicts.clone()))));
    registry
}

pub fn run(setup: Setup) -> Trace {
    let provider = ScriptedProvider::new(setup.turns);
    let store = Rc::new(InMemoryStore::default());
    let flag = StopFlag::default();
    let frontend = Rc::new(StoppingFrontend {
        inner: RecordingFrontend { answers: std::cell::RefCell::new(VecDeque::from(setup.answers)), ..Default::default() },
        after_chunks: setup.stop_at_turn.filter(|n| *n > 0).map(Cell::new),
        flag: flag.clone(),
    });

    let parts = registry(provider.clone(), store.clone(), setup.decision, setup.max_turns, setup.hook, setup.verdicts).resolve(&selection(), platform()).expect("the scenario's selection resolves");

    let engine = Engine::new(parts, frontend.clone(), "trace", ".").with_stop_flag(flag.clone());
    let protocol = setup.protocol.then(|| Protocol { steps: vec!["cargo test".into()], specs: vec![] });
    let mut ends = Vec::new();
    for (i, prompt) in setup.prompts.into_iter().enumerate() {
        // A stop "before the run" comes while the first turn is starting: from a hook's event.
        if i == 0 && setup.stop_at_turn == Some(0) {
            frontend.after_chunks_zero(&flag);
        }
        ends.push(pollster::block_on(engine.run(prompt, protocol.as_ref())));
    }
    let stored = store.sessions.borrow().get("trace").cloned().unwrap_or_default();
    let entries = frontend.inner.entries.borrow().clone();
    let requests = provider.requests.borrow().clone();
    Trace { entries, stored, ends, state: engine.state(), asked: frontend.inner.asked.get(), requests }
}

/// The recording frontend, plus the user pressing Stop at a chosen moment.
struct StoppingFrontend {
    inner: RecordingFrontend,
    after_chunks: Option<Cell<usize>>,
    flag: StopFlag,
}

impl StoppingFrontend {
    fn after_chunks_zero(&self, _flag: &StopFlag) {
        STOP_ON_PROMPT.with(|s| s.set(true));
    }
}

thread_local! {
    static STOP_ON_PROMPT: Cell<bool> = const { Cell::new(false) };
}

impl Frontend for StoppingFrontend {
    fn on_entry(&self, entry: &LogEntry) {
        self.inner.on_entry(entry);
        if matches!(entry.event, Event::UserMessage { .. }) && STOP_ON_PROMPT.with(|s| s.replace(false)) {
            self.flag.stop();
        }
        if let (Event::TextDelta { .. }, Some(left)) = (&entry.event, &self.after_chunks) {
            left.set(left.get().saturating_sub(1));
            if left.get() == 0 {
                self.flag.stop();
            }
        }
    }

    fn ask<'a>(&'a self, call: &'a ToolCall, capabilities: &'a [Capability]) -> BoxFuture<'a, Answer> {
        self.inner.ask(call, capabilities)
    }
}

/// One log entry as one readable line.
pub fn line(entry: &LogEntry) -> String {
    let body = match &entry.event {
        Event::UserMessage { text } => format!("user        {text:?}"),
        Event::Inbound { label, text } => format!("inbound     {label}: {text:?}"),
        Event::TaskState { to, reason } => format!("task        -> {to:?}{}", reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()),
        Event::TurnStarted { turn, provider, model } => format!("turn {turn}      start {provider}/{model}"),
        Event::TextDelta { text } => format!("text        {text:?}"),
        Event::ThinkingDelta { text } => format!("thinking    {text:?}"),
        Event::OpaqueBlock { block } => format!("block       {block}"),
        Event::ModelToolCall { call } => format!("call        {} {} {}", call.id, call.name, call.input),
        Event::TurnFinished { turn, stop, usage, cost_micro_usd } => {
            format!("turn {turn}      end {stop:?}, {} in / {} out, {} µ$", usage.input, usage.output, cost_micro_usd.unwrap_or(0))
        }
        Event::ToolRequested { call, capabilities } => format!("tool        {} {} needs {capabilities:?}", call.id, call.name),
        Event::ToolDecision { id, allowed, by, reason } => {
            format!("decision    {id} {} by {by:?}{}", if *allowed { "allowed" } else { "denied" }, reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default())
        }
        Event::ToolResult { id, output, is_error } => format!("result      {id}{} {output:?}", if *is_error { " ERROR" } else { "" }),
        Event::HookRan { point, hook, outcome, .. } => format!("hook        {point:?} {hook}: {outcome:?}"),
        Event::Verification { pass, evidence } => format!("verify      {} {evidence:?}", if *pass { "pass" } else { "FAIL" }),
        Event::BackendSession { id } => format!("backend     session {id}"),
        Event::AgentName { name, by } => format!("agent name  {name} by {by}"),
        Event::StepDebug { on, by } => format!("step-debug  {} by {by}", if *on { "on" } else { "off" }),
        Event::PlanMode { on, by } => format!("plan mode   {} by {by}", if *on { "on" } else { "off" }),
        Event::Compacted { upto, tokens_before, tokens_after } => format!("compacted   before message {upto}: {tokens_before} -> {tokens_after} tokens"),
        Event::Error { source, message } => format!("error       {source}: {message}"),
        Event::BackendCost { total_micro_usd } => format!("cost        {total_micro_usd} in all"),
        Event::ConversationImported { source, id, .. } => format!("imported    {source} {id}"),
    };
    format!("{:>4} {:>6}ms  {body}", entry.seq, entry.at_ms)
}
