//! Session state is a fold over the log (INV-5): nothing here holds what the log does not.

use serde::{Deserialize, Serialize};

use crate::event::{Event, HookOutcome};
use crate::model::{Content, Message, Role, Usage};

/// IF-0.4. `done` is reachable only through `verifying` (INV-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    #[default]
    Planned,
    Running,
    Verifying,
    Done,
    Failed,
    Abandoned,
}

impl TaskState {
    /// Whether the lifecycle allows this step. A failed verification goes back to running.
    pub fn can_become(self, to: TaskState) -> bool {
        use TaskState::*;
        matches!(
            (self, to),
            (Planned, Running)
                | (Running, Verifying)
                | (Verifying, Done)
                | (Verifying, Running)
                | (Planned | Running | Verifying, Failed | Abandoned)
        )
    }

    pub fn is_final(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed | TaskState::Abandoned)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionState {
    pub task: TaskState,
    pub messages: Vec<Message>,
    pub usage: Usage,
    pub cost_micro_usd: u64,
    pub turns: u32,
    /// Lines hooks added to the context (IF-6).
    pub hook_context: Vec<String>,
    /// The evidence of the last verification.
    pub evidence: Option<String>,
}

impl SessionState {
    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::UserMessage { text } => self.messages.push(Message::user(text.clone())),
            Event::TaskState { to, .. } => self.task = *to,
            Event::TurnStarted { turn, .. } => self.turns = *turn,
            Event::TextDelta { text } => match self.assistant_tail().last_mut() {
                Some(Content::Text { text: t }) => t.push_str(text),
                _ => self.assistant_tail().push(Content::Text { text: text.clone() }),
            },
            Event::TurnFinished { usage, cost_micro_usd, .. } => {
                self.usage.add(*usage);
                self.cost_micro_usd += cost_micro_usd.unwrap_or(0);
            }
            Event::ToolRequested { call, .. } => self.assistant_tail().push(Content::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            }),
            Event::ToolDecision { .. } => {}
            Event::ToolResult { id, output, is_error } => {
                let result = Content::ToolResult { id: id.clone(), output: output.clone(), is_error: *is_error };
                // The results of one turn travel together in one user message.
                match self.messages.last_mut() {
                    Some(m) if m.role == Role::User && matches!(m.content.last(), Some(Content::ToolResult { .. })) => m.content.push(result),
                    _ => self.messages.push(Message { role: Role::User, content: vec![result] }),
                }
            }
            Event::HookRan { outcome, .. } => {
                if let HookOutcome::Context { lines } = outcome {
                    self.hook_context.extend(lines.iter().cloned());
                }
            }
            Event::Verification { evidence, .. } => self.evidence = Some(evidence.clone()),
            Event::Error { .. } => {}
        }
    }

    pub fn fold<'a>(events: impl IntoIterator<Item = &'a Event>) -> Self {
        let mut state = SessionState::default();
        for e in events {
            state.apply(e);
        }
        state
    }

    /// The content of the assistant message being written, opened if there is none.
    fn assistant_tail(&mut self) -> &mut Vec<Content> {
        if self.messages.last().is_none_or(|m| m.role != Role::Assistant) {
            self.messages.push(Message { role: Role::Assistant, content: Vec::new() });
        }
        &mut self.messages.last_mut().expect("just pushed").content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StopReason, ToolCall};
    use proptest::prelude::*;

    fn event() -> impl Strategy<Value = Event> {
        prop_oneof![
            "[a-z ]{0,8}".prop_map(|text| Event::UserMessage { text }),
            "[a-z ]{0,8}".prop_map(|text| Event::TextDelta { text }),
            (0u32..9, 0u64..99).prop_map(|(turn, n)| Event::TurnFinished {
                turn,
                stop: StopReason::EndTurn,
                usage: Usage { input: n, output: n, cache_read: 0, cache_write: 0 },
                cost_micro_usd: Some(n),
            }),
            "[a-z]{1,4}".prop_map(|id| Event::ToolRequested {
                call: ToolCall { id, name: "t".into(), input: serde_json::json!({}) },
                capabilities: vec![],
            }),
            ("[a-z]{1,4}", "[a-z ]{0,8}", any::<bool>()).prop_map(|(id, output, is_error)| Event::ToolResult { id, output, is_error }),
            "[a-z ]{0,8}".prop_map(|evidence| Event::Verification { pass: false, evidence }),
        ]
    }

    proptest! {
        /// INV-5: applying events one by one is the fold of the log, and a rewind to event N
        /// is the fold of the first N events.
        #[test]
        fn state_is_a_fold_of_the_log(events in proptest::collection::vec(event(), 0..40), n in 0usize..40) {
            let mut live = SessionState::default();
            let mut at_n = SessionState::default();
            let cut = n.min(events.len());
            for (i, e) in events.iter().enumerate() {
                if i == cut {
                    at_n = live.clone();
                }
                live.apply(e);
            }
            if cut == events.len() {
                at_n = live.clone();
            }
            prop_assert_eq!(&SessionState::fold(&events), &live);
            prop_assert_eq!(&SessionState::fold(&events[..cut]), &at_n);
            // The log survives its own serialisation, so a replay from disk is the same fold.
            let text = serde_json::to_string(&events).unwrap();
            let back: Vec<Event> = serde_json::from_str(&text).unwrap();
            prop_assert_eq!(&SessionState::fold(&back), &live);
        }
    }

    #[test]
    fn done_only_through_verifying() {
        use TaskState::*;
        assert!(!Running.can_become(Done));
        assert!(!Planned.can_become(Done));
        assert!(Verifying.can_become(Done));
        assert!(Verifying.can_become(Running));
        assert!(!Done.can_become(Running));
    }
}
