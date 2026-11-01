//! The loop's behaviour (IF-0.1, IF-0.2), run through the named scenarios.

use harness_api::event::DecidedBy;
use harness_api::*;
use harness_core::RunEnd;
use harness_scenarios::{NAMES, Trace, run, setup};

fn scenario(name: &str) -> Trace {
    run(setup(name).expect("known scenario"))
}

fn tool_result(t: &Trace) -> (String, bool) {
    t.entries.iter().find_map(|e| match &e.event {
        Event::ToolResult { output, is_error, .. } => Some((output.clone(), *is_error)),
        _ => None,
    }).expect("a tool result")
}

fn decision(t: &Trace) -> (bool, DecidedBy) {
    t.entries.iter().find_map(|e| match &e.event {
        Event::ToolDecision { allowed, by, .. } => Some((*allowed, *by)),
        _ => None,
    }).expect("a decision")
}

#[test]
fn plain_answer_waits_for_input_and_is_never_done() {
    let t = scenario("plain");
    assert_eq!(t.ends, vec![RunEnd::AwaitingInput]);
    assert_eq!(t.state.task, TaskState::Running);
    assert_eq!(t.state.messages.len(), 2);
    assert_eq!(t.state.messages[1].content, vec![Content::Text { text: "Hello there, human.".into() }]);
}

#[test]
fn allowed_tool_runs_and_the_model_gets_its_result() {
    let t = scenario("tool-allow");
    assert_eq!(tool_result(&t), ("hello from the tool".to_string(), false));
    assert_eq!(decision(&t), (true, DecidedBy::Policy));
    let second = &t.requests[1];
    assert!(matches!(second.messages.last().unwrap().content[0], Content::ToolResult { .. }));
    assert_eq!(t.requests[0].tools[0].name, "echo");
}

#[test]
fn denied_tool_does_not_run_and_the_model_is_told() {
    let t = scenario("tool-deny");
    let (output, is_error) = tool_result(&t);
    assert!(is_error && output.contains("not in this project"), "{output}");
    assert_eq!(decision(&t), (false, DecidedBy::Policy));
    assert_eq!(t.ends, vec![RunEnd::AwaitingInput]);
}

#[test]
fn asked_tool_follows_the_users_answer() {
    let yes = scenario("tool-ask-yes");
    assert_eq!(yes.asked, 1);
    assert_eq!(decision(&yes), (true, DecidedBy::User));
    assert!(!tool_result(&yes).1);
    let no = scenario("tool-ask-no");
    assert_eq!(no.asked, 1);
    assert_eq!(decision(&no), (false, DecidedBy::User));
    assert!(tool_result(&no).1);
}

#[test]
fn unknown_tool_is_an_error_result_not_a_crash() {
    let t = scenario("unknown-tool");
    assert!(tool_result(&t).0.contains("no-such-tool"));
    assert_eq!(t.ends, vec![RunEnd::AwaitingInput]);
}

#[test]
fn a_hook_can_veto_a_tool_and_add_context() {
    let veto = scenario("hook-veto");
    assert_eq!(decision(&veto), (false, DecidedBy::Hook));
    assert!(tool_result(&veto).0.contains("no tests were run"));
    let context = scenario("hook-context");
    assert_eq!(context.requests[0].system.as_deref(), Some("Journal your decisions."));
    let prompt = scenario("prompt-veto");
    assert!(matches!(&prompt.ends[0], RunEnd::Failed(reason) if reason == "not today"));
    assert!(prompt.requests.is_empty(), "a vetoed prompt reaches no model");
}

/// INV-4: done only through a passing verifier.
#[test]
fn done_means_proven() {
    let pass = scenario("verify-pass");
    assert_eq!(pass.ends, vec![RunEnd::Done]);
    assert_eq!(pass.state.task, TaskState::Done);
    assert_eq!(pass.state.evidence.as_deref(), Some("cargo test: 3 passed"));

    let retry = scenario("verify-fail-then-pass");
    assert_eq!(retry.ends, vec![RunEnd::Done]);
    let states: Vec<_> = retry.entries.iter().filter_map(|e| match &e.event {
        Event::TaskState { to, .. } => Some(*to),
        _ => None,
    }).collect();
    use TaskState::*;
    assert_eq!(states, vec![Running, Verifying, Running, Verifying, Done]);
    let fed_back = format!("{:?}", retry.requests[1].messages);
    assert!(fed_back.contains("cargo test: 1 failed"), "the model sees the failed evidence");
}

/// INV-4: a verifier that never passes never reaches done; the budget ends it.
#[test]
fn a_verifier_that_keeps_failing_never_reaches_done() {
    let mut s = setup("verify-pass").unwrap();
    s.verdicts = vec![];
    s.max_turns = Some(3);
    let t = run(s);
    assert!(matches!(&t.ends[0], RunEnd::Failed(reason) if reason.contains("budget")));
    assert_eq!(t.state.task, TaskState::Failed);
    assert!(!t.entries.iter().any(|e| matches!(e.event, Event::TaskState { to: TaskState::Done, .. })));
}

#[test]
fn a_budget_ends_the_task() {
    let t = scenario("budget");
    assert!(matches!(&t.ends[0], RunEnd::Failed(reason) if reason.contains("budget")));
    assert_eq!(t.state.turns, 2);
}

#[test]
fn the_user_can_stop_it() {
    let t = scenario("stop");
    assert_eq!(t.ends, vec![RunEnd::Abandoned]);
    assert_eq!(t.state.task, TaskState::Abandoned);
    assert!(t.requests.is_empty());
}

#[test]
fn a_provider_error_fails_the_task_with_its_message() {
    let t = scenario("provider-error");
    assert_eq!(t.ends, vec![RunEnd::Failed("overloaded".into())]);
    assert_eq!(t.state.task, TaskState::Failed);
}

#[test]
fn a_second_prompt_continues_the_conversation() {
    let t = scenario("two-prompts");
    assert_eq!(t.ends, vec![RunEnd::AwaitingInput, RunEnd::AwaitingInput]);
    assert_eq!(t.requests[1].messages.len(), 3);
    let starts = t.entries.iter().filter(|e| matches!(e.event, Event::HookRan { point: harness_api::event::HookPointKind::SessionStart, .. })).count();
    assert_eq!(starts, 1, "SessionStart runs once");
}

/// INV-5, on real runs: the store holds exactly what the frontend saw, numbered without gaps,
/// and replaying it gives the engine's state.
#[test]
fn every_scenario_logs_everything_and_replays_to_its_state() {
    for name in NAMES {
        let t = scenario(name);
        assert_eq!(t.stored, t.entries, "{name}: store and frontend differ");
        for (i, e) in t.stored.iter().enumerate() {
            assert_eq!(e.seq, i as u64, "{name}: gap in the log");
        }
        let replayed = SessionState::fold(t.stored.iter().map(|e| &e.event));
        assert_eq!(replayed, t.state, "{name}: replay differs");
    }
}

/// INV-6: the double's credential never reaches a log.
#[test]
fn the_canary_secret_never_appears_in_a_log() {
    for name in NAMES {
        let t = scenario(name);
        let log = serde_json::to_string(&t.stored).unwrap();
        assert!(!log.contains("canary-secret"), "{name}");
    }
    assert_eq!(format!("{:?} {}", Secret::new("canary-secret"), Secret::new("canary-secret")), "[redacted] [redacted]");
}

/// INV-5: a stored line reads back as the same entry.
#[test]
fn log_lines_round_trip() {
    for name in NAMES {
        for entry in scenario(name).stored {
            let back = harness_api::event::from_line(&harness_api::event::to_line(&entry)).unwrap();
            assert_eq!(back, entry, "{name}");
        }
    }
}
