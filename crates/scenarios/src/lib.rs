//! Named runs of the real loop on test doubles. A scenario names only doubles in its
//! `Selection`, so it also proves INV-2: no default plugin is linked. `multitool trace <name>`
//! prints a scenario's log line by line, the debug view of the loop.

use std::collections::VecDeque;
use std::rc::Rc;

use harness_api::event::{HookOutcome, HookPointKind};
use harness_api::*;
use harness_core::{Engine, Registry, RunEnd, Selection};
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
    pub stop_before_run: bool,
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
    "provider-error",
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
        "stop" => Setup { turns: vec![done()], stop_before_run: true, ..s },
        "provider-error" => Setup { turns: vec![ScriptedTurn::fail("overloaded")], ..s },
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

pub fn run(setup: Setup) -> Trace {
    let provider = ScriptedProvider::new(setup.turns);
    let store = Rc::new(InMemoryStore::default());
    let frontend = Rc::new(RecordingFrontend { answers: std::cell::RefCell::new(VecDeque::from(setup.answers)), ..Default::default() });

    let mut registry = Registry::new();
    let (p, st) = (provider.clone(), store.clone());
    let decision = setup.decision.unwrap_or(Decision::Allow);
    let max_turns = setup.max_turns.unwrap_or(50);
    let (hook, verdicts) = (setup.hook, setup.verdicts);
    registry
        .auth("double", |_| Ok(Rc::new(FixedAuth(Secret::new("canary-secret")))))
        .provider("scripted", move |_| Ok(p.clone()))
        .router("double", move |_| Ok(Rc::new(OneModelRouter { provider: "scripted".into(), max_turns })))
        .tool("echo", |_| Ok(Rc::new(EchoTool { name: "echo".into(), needs: vec![Capability::Command("echo".into())] })))
        .policy("double", move |_| Ok(Rc::new(FixedPolicy::new(decision.clone()))))
        .hook("double", move |_| {
            let (on, outcome) = hook.clone().unwrap_or((HookPointKind::Stop, HookOutcome::Ok));
            Ok(Rc::new(FixedHook { name: "fixture.sh".into(), on, outcome }))
        })
        .memory("double", |_| Ok(Rc::new(InMemoryJournal::default())))
        .context("double", |_| Ok(Rc::new(WholeConversation)))
        .store("double", move |_| Ok(st.clone()))
        .verifier("double", move |_| Ok(Rc::new(ScriptedVerifier::new(verdicts.clone()))));
    let parts = registry.resolve(&selection(), platform()).expect("the scenario's selection resolves");

    let engine = Engine::new(parts, frontend.clone(), "trace", ".");
    if setup.stop_before_run {
        engine.stop_flag().stop();
    }
    let protocol = setup.protocol.then(|| Protocol { steps: vec!["cargo test".into()] });
    let mut ends = Vec::new();
    for prompt in setup.prompts {
        ends.push(pollster::block_on(engine.run(prompt, protocol.as_ref())));
    }
    let stored = store.sessions.borrow().get("trace").cloned().unwrap_or_default();
    let entries = frontend.entries.borrow().clone();
    let requests = provider.requests.borrow().clone();
    Trace { entries, stored, ends, state: engine.state(), asked: frontend.asked.get(), requests }
}

/// One log entry as one readable line.
pub fn line(entry: &LogEntry) -> String {
    let body = match &entry.event {
        Event::UserMessage { text } => format!("user        {text:?}"),
        Event::TaskState { to, reason } => format!("task        -> {to:?}{}", reason.as_ref().map(|r| format!(" ({r})")).unwrap_or_default()),
        Event::TurnStarted { turn, provider, model } => format!("turn {turn}      start {provider}/{model}"),
        Event::TextDelta { text } => format!("text        {text:?}"),
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
        Event::Error { source, message } => format!("error       {source}: {message}"),
    };
    format!("{:>4} {:>6}ms  {body}", entry.seq, entry.at_ms)
}
