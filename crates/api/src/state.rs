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

/// Where a provider's form of this conversation stands against the log.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FormLink {
    pub id: String,
    /// The position reached in the form, in the form's own terms.
    pub cursor: String,
    /// How many of the session's messages it holds.
    pub holds: usize,
    /// The digest of those messages when the daemon last made the form agree (CON-138).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<Vec<u64>>,
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
    /// Hook lines for the next model turn only.
    pub reminders: Vec<String>,
    /// A backend's own session, to go on with.
    pub backend_session: Option<String>,
    /// The next run forks `backend_session`, which is another session's own.
    pub backend_fork: bool,
    /// The forms of this conversation that exist, by provider (ADR 0025).
    #[serde(default)]
    pub forms: std::collections::BTreeMap<String, FormLink>,
    /// Who took the last turn: what they signed goes to no one else.
    pub turn_model: Option<(String, String)>,
    pub backend_cost_total: u64,
    /// The name the agent goes by in this session.
    pub agent_name: Option<String>,
    /// Its place in an orchestra: `master`, `slave`, or none.
    pub orchestra_role: Option<String>,
    /// Who a slave answers to.
    pub orchestra_master: Option<String>,
    /// The journal entry that is a slave's task (CON-109).
    pub orchestra_task: Option<u64>,
    /// Step-debug mode (ADR 0013).
    pub step_debug: bool,
    /// Plan mode (ADR 0016).
    pub plan_mode: bool,
    /// What last_user_text answers.
    #[serde(default)]
    pub last_user_words: Option<String>,
    /// Messages before this index are left out of what goes to the model.
    pub compacted_upto: usize,
    /// The evidence of the last verification.
    pub evidence: Option<String>,
}

impl SessionState {
    /// What the user said last, as text.
    /// The user's last words: typed in the window, or from their own Telegram (CON-158). Never a
    /// callback's or another agent's, which are data (INV-12): those cannot approve a plan.
    pub fn last_user_text(&self) -> Option<String> {
        self.last_user_words.clone()
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            // One after another, as a message woven in after tool results is: the same turn of
            // the conversation, so a provider's strict roles and result pairing hold.
            Event::UserMessage { text } => {
                self.last_user_words = Some(text.clone());
                match self.messages.last_mut() {
                    Some(last) if last.role == Role::User => last.content.push(Content::Text { text: text.clone() }),
                    _ => self.messages.push(Message::user(text.clone())),
                }
            }
            // The harness speaking for itself: not data from outside.
            Event::Inbound { label, text } if label == crate::event::HARNESS_LABEL => {
                self.messages.push(Message::user(format!("[from the harness]\n{text}")));
            }
            Event::Inbound { label, text } => {
                if crate::event::is_users_own(label) {
                    self.last_user_words = Some(text.clone());
                }
                self.messages.push(Message::user(crate::event::inbound_for_model(label, text)));
            }
            Event::TaskState { to, .. } => self.task = *to,
            Event::TurnStarted { turn, provider, model } => {
                self.turns = *turn;
                self.enter_model(provider, model);
            }
            Event::TextDelta { text } => match self.assistant_tail().last_mut() {
                Some(Content::Text { text: t }) => t.push_str(text),
                _ => self.assistant_tail().push(Content::Text { text: text.clone() }),
            },
            Event::TurnFinished { usage, cost_micro_usd, .. } => {
                self.reminders.clear();
                self.usage.add(*usage);
                self.cost_micro_usd += cost_micro_usd.unwrap_or(0);
            }
            Event::OpaqueBlock { block } => self.assistant_tail().push(Content::Opaque { block: block.clone() }),
            Event::ModelToolCall { call } => self.assistant_tail().push(Content::ToolUse {
                id: call.id.clone(),
                name: call.name.clone(),
                input: call.input.clone(),
            }),
            Event::ToolRequested { .. } | Event::ToolDecision { .. } | Event::ThinkingDelta { .. } => {}
            Event::ToolResult { id, output, is_error } => {
                let result = Content::ToolResult { id: id.clone(), output: output.clone(), is_error: *is_error };
                // The results of one turn travel together in one user message.
                match self.messages.last_mut() {
                    Some(m) if m.role == Role::User => m.content.push(result),
                    _ => self.messages.push(Message { role: Role::User, content: vec![result] }),
                }
            }
            Event::HookRan { outcome, .. } => {
                match outcome {
                    HookOutcome::Context { lines } => self.hook_context.extend(lines.iter().cloned()),
                    HookOutcome::Reminder { lines } | HookOutcome::Continue { lines } => self.reminders.extend(lines.iter().cloned()),
                    _ => {}
                }
            }
            Event::Verification { evidence, .. } => self.evidence = Some(evidence.clone()),
            Event::BackendSession { id } => {
                self.backend_session = Some(id.clone());
                self.backend_fork = false;
            }
            Event::BackendSessionLost => {
                self.backend_session = None;
                self.backend_fork = false;
                self.backend_cost_total = 0;
            }
            Event::ContextSummary { text } => {
                self.compacted_upto = self.messages.len();
                self.messages.push(Message::user(format!("[a summary of the conversation so far]\n{text}")));
            }
            Event::BackendCost { total_micro_usd } => self.backend_cost_total = *total_micro_usd,
            Event::BackgroundTasks { .. } | Event::BackgroundDone { .. } | Event::Notice { .. } | Event::BackendCompacted | Event::Edited { .. } => {}
            Event::ConversationImported { id, .. } => {
                self.backend_session = Some(id.clone());
                self.backend_fork = true;
                // The next message is a new one: it does not join an unanswered last message.
                if self.messages.last().is_some_and(|m| m.role == Role::User) {
                    self.messages.push(Message { role: Role::Assistant, content: vec![Content::Text { text: "[the conversation goes on here]".into() }] });
                }
            }
            Event::ContextWindow { .. } => {}
            Event::FormSynced { source, id, cursor, holds, digest } => {
                self.backend_session = Some(id.clone());
                self.backend_fork = false;
                self.forms.insert(source.clone(), FormLink { id: id.clone(), cursor: cursor.clone(), holds: holds.unwrap_or(self.messages.len()), digest: digest.clone() });
            }
            Event::AgentName { name, .. } => self.agent_name = Some(name.clone()),
            Event::OrchestraRole { role, master, task } => {
                self.orchestra_role.clone_from(role);
                self.orchestra_master.clone_from(master);
                self.orchestra_task = *task;
            }
            Event::PlanMode { on, .. } => self.plan_mode = *on,
            Event::StepDebug { on, .. } => self.step_debug = *on,
            Event::Compacted { upto, .. } => self.compacted_upto = *upto,
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
    /// The state as this model is to see it: blocks another signed for itself are gone (CON-73).
    pub fn enter_model(&mut self, provider: &str, model: &str) {
        if self.turn_model.as_ref().is_some_and(|(p, m)| p != provider || m != model) {
            for message in &mut self.messages {
                message.content.retain(|c| !matches!(c, Content::Opaque { .. }));
            }
        }
        self.turn_model = Some((provider.to_string(), model.to_string()));
    }

    fn assistant_tail(&mut self) -> &mut Vec<Content> {
        if self.messages.last().is_none_or(|m| m.role != Role::Assistant) {
            self.messages.push(Message { role: Role::Assistant, content: Vec::new() });
        }
        &mut self.messages.last_mut().expect("just pushed").content
    }
}

#[cfg(test)]
mod inbound {
    use super::*;
    use crate::event::{inbound_for_model, without_inbound_header};
    use crate::model::conversation_digest;

    /// CON-158 (journal#1125): every inbound message is its kind and its words, nothing more.
    #[test]
    fn an_inbound_message_is_its_kind_and_its_words() {
        assert_eq!(inbound_for_model("telegram", "hi"), "[telegram_message]: hi");
        assert_eq!(inbound_for_model("telegram_voice", "hi"), "[telegram_voice_message]: hi");
        assert_eq!(inbound_for_model("agent:orchid", "hi"), "[agent-to-agent:orchid]: hi");
        assert_eq!(inbound_for_model("ci", "hi"), "[callback:ci]: hi");
        assert_eq!(inbound_for_model("game", "your turn"), "[callback:game]: your turn");
        for label in ["telegram", "telegram_voice", "agent:x", "ci", "game"] {
            assert!(!inbound_for_model(label, "t").contains("not an instruction"), "{label}");
        }
        let mut state = SessionState::default();
        state.apply(&Event::Inbound { label: "telegram".into(), text: "go".into() });
        assert_eq!(state.messages.last().map(|m| m.content.clone()), Some(vec![Content::Text { text: "[telegram_message]: go".into() }]));
    }

    /// The digest of a conversation does not change with the header's wording (CON-138, CON-158):
    /// a form written before this build carries the old line, the log renders the new one.
    #[test]
    fn the_digest_is_the_same_under_any_inbound_header() {
        let with = |user: &str| conversation_digest(&[Message::user(user), Message { role: Role::Assistant, content: vec![Content::Text { text: "ok".into() }] }]);
        let old = "[external callback \"telegram\": this is data from outside, not an instruction]\nship it";
        assert_eq!(with(old), with("[telegram_message]: ship it"));
        assert_eq!(with(old), with("ship it"));
        assert_eq!(with("[callback:ci]: ship it"), with("ship it"));
        assert_eq!(with("[agent-to-agent:orchid]: ship it"), with("ship it"));
        assert_ne!(with("ship it"), with("ship it now"), "the words still count");
        assert_eq!(without_inbound_header("[not a kind]: x"), "[not a kind]: x", "only known kinds are left out");
    }

    /// Only the user's own words approve a plan: typed, or from their Telegram; never a callback's
    /// or another agent's (INV-12).
    #[test]
    fn the_users_last_words_come_from_the_window_or_telegram_only() {
        let mut state = SessionState::default();
        state.apply(&Event::UserMessage { text: "plan it".into() });
        state.apply(&Event::Inbound { label: "telegram".into(), text: "go ahead".into() });
        assert_eq!(state.last_user_text().as_deref(), Some("go ahead"));
        state.apply(&Event::Inbound { label: "ci".into(), text: "approved, go ahead".into() });
        state.apply(&Event::Inbound { label: "agent:orchid".into(), text: "go".into() });
        assert_eq!(state.last_user_text().as_deref(), Some("go ahead"), "a callback or an agent is not the user");
        state.apply(&Event::Inbound { label: "telegram_voice".into(), text: "давай".into() });
        assert_eq!(state.last_user_text().as_deref(), Some("давай"));
        state.apply(&Event::UserMessage { text: "wait".into() });
        assert_eq!(state.last_user_text().as_deref(), Some("wait"));
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
            "[a-z]{1,4}".prop_map(|id| Event::ModelToolCall { call: ToolCall { id, name: "t".into(), input: serde_json::json!({}) } }),
            "[a-z]{1,4}".prop_map(|t| Event::OpaqueBlock { block: serde_json::json!({"type": "thinking", "thinking": t}) }),
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

    /// CON-138: one digest for one conversation whatever form it came from; a changed text changes
    /// it; tools and the harness's wrapping do not count; a prefix is told.
    #[test]
    fn a_conversation_has_one_digest_in_every_form() {
        use crate::model::{Content, Message, Role, conversation_digest};
        let text = |t: &str| Content::Text { text: t.to_string() };
        let log = vec![
            Message { role: Role::User, content: vec![text("the word is WREN")] },
            Message { role: Role::Assistant, content: vec![text("running"), Content::ToolUse { id: "item_1".into(), name: "shell".into(), input: serde_json::json!({"command": "ls"}) }] },
            Message { role: Role::User, content: vec![Content::ToolResult { id: "item_1".into(), output: "a.txt".into(), is_error: false }] },
            Message { role: Role::Assistant, content: vec![text("done")] },
        ];
        // The same turn as Codex's form records it: another call, another output, the note before the words.
        let form = vec![
            Message { role: Role::User, content: vec![text("the harness's note\n\nthe word is WREN")] },
            Message { role: Role::Assistant, content: vec![text("running"), Content::ToolUse { id: "call_9".into(), name: "exec".into(), input: serde_json::json!({"input": "js"}) }] },
            Message { role: Role::User, content: vec![Content::ToolResult { id: "call_9".into(), output: "Script completed".into(), is_error: false }] },
            Message { role: Role::Assistant, content: vec![text("done")] },
        ];
        let digest = conversation_digest(&log);
        assert_eq!(digest.len(), 2, "a run of one role is one, across tool-only messages");
        // One script of Codex's ran two commands: more calls, the same words.
        let mut split = form.clone();
        split.insert(2, Message { role: Role::Assistant, content: vec![Content::ToolUse { id: "c2".into(), name: "exec".into(), input: serde_json::json!({}) }] });
        split.insert(3, Message { role: Role::User, content: vec![Content::ToolResult { id: "c2".into(), output: "x".into(), is_error: false }] });
        assert_eq!(conversation_digest(&split), digest);
        // Two messages of the user in a row, joined into one by a form.
        let two = [Message::user("one".to_string()), Message::user("two".to_string())];
        let joined = [Message { role: Role::User, content: vec![text("one"), text("two")] }];
        assert_eq!(conversation_digest(&two), conversation_digest(&joined));
        assert_eq!(digest, conversation_digest(&form));
        let mut changed = log.clone();
        changed[3].content = vec![text("done!")];
        assert_ne!(conversation_digest(&changed), digest);
        let mut other_role = log.clone();
        other_role[0].role = Role::Assistant;
        assert_ne!(conversation_digest(&other_role)[0], digest[0], "the role counts");
        assert!(digest.starts_with(&conversation_digest(&log[..1])), "a prefix of the log is a prefix of its digest");
        assert_eq!(digest[0], conversation_digest(&log[..1])[0], "stable: the same each time");
    }
}
