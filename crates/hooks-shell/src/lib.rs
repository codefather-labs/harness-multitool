//! A hook that runs a shell command (IF-6). The command sees `HOOK_EVENT`, and for a tool event
//! `HOOK_CALL` (the call as JSON). Exit 0: its stdout lines join the agent's context. Exit 2: a
//! veto, with stderr as the reason. Anything else: failed, and it blocks nothing.

use std::rc::Rc;

use harness_api::event::{HookOutcome, HookPointKind};
use harness_api::platform::Process;
use harness_api::*;
use serde_json::json;

pub struct ShellHook {
    name: String,
    on: Option<HookPointKind>,
    command: String,
    root: String,
    process: Option<Rc<dyn Process>>,
}

/// The event name in the configuration.
pub fn point_kind(name: &str) -> Option<HookPointKind> {
    Some(match name {
        "SessionStart" => HookPointKind::SessionStart,
        "UserPromptSubmit" => HookPointKind::UserPromptSubmit,
        "PreToolUse" => HookPointKind::PreToolUse,
        "PostToolUse" => HookPointKind::PostToolUse,
        "Stop" => HookPointKind::Stop,
        "PostCompact" => HookPointKind::PostCompact,
        "PlanEnter" => HookPointKind::PlanEnter,
        _ => return None,
    })
}

impl ShellHook {
    /// `event: None` runs on every event.
    pub fn new(name: &str, event: Option<HookPointKind>, command: &str, root: &str, process: Option<Rc<dyn Process>>) -> Self {
        ShellHook { name: name.to_string(), on: event, command: command.to_string(), root: root.to_string(), process }
    }
}

impl Hook for ShellHook {
    fn name(&self) -> &str {
        &self.name
    }

    fn applies(&self, point: &HookPoint<'_>) -> bool {
        self.on.is_none_or(|on| on == point.kind())
    }

    fn run<'a>(&'a self, point: &'a HookPoint<'a>) -> BoxFuture<'a, HookOutcome> {
        Box::pin(async move {
            let Some(process) = &self.process else {
                return HookOutcome::Failed { message: "this platform has no processes".into() };
            };
            let mut env = vec![("HOOK_EVENT".to_string(), format!("{:?}", point.kind()))];
            match point {
                HookPoint::PreToolUse { call } | HookPoint::PostToolUse { call, .. } => {
                    env.push(("HOOK_CALL".to_string(), json!({"name": call.name, "input": call.input}).to_string()));
                }
                HookPoint::UserPromptSubmit { text } => env.push(("HOOK_PROMPT".to_string(), text.to_string())),
                _ => {}
            }
            match process.run(&self.command, Some(&self.root), &env).await {
                Ok(out) => match out.code {
                    Some(0) => {
                        let lines: Vec<String> = out.stdout.lines().map(str::to_string).collect();
                        if lines.is_empty() { HookOutcome::Ok } else { HookOutcome::Context { lines } }
                    }
                    Some(2) => HookOutcome::Blocked { reason: out.stderr.trim().to_string() },
                    code => HookOutcome::Failed { message: format!("exit {}: {}", code.map_or("none".to_string(), |c| c.to_string()), out.stderr.trim()) },
                },
                Err(e) => HookOutcome::Failed { message: e.to_string() },
            }
        })
    }
}
