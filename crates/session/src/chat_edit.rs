//! Editing a conversation from Settings > Chats (CON-189): its log in the cards the chat draws,
//! and a copy saved without the cards taken out.
//!
//! The session log is append-only (INV-5): an edit never rewrites it. Saving writes a new
//! conversation beside the original, with the kept events in their order. Its links to the forms
//! of other programs (the Claude Code transcript, the Codex rollout) are left out, so its first
//! message writes that form fresh from the edited messages (`Engine::hand_over`).

use std::collections::{HashMap, HashSet};

use harness_api::{Event, LogEntry, SessionState};

/// What a card of the editor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// A message of the user.
    User,
    /// A message that came from outside: Telegram, a callback, another agent, the harness.
    Inbound,
    /// What the agent said between two messages, with the tool calls it made (its `children`).
    Answer,
    /// One tool call, with its request, decision and result.
    Tool,
}

/// One card: the events it owns, by their `seq`, and the cards that go with it when it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub kind: CardKind,
    pub seqs: Vec<u64>,
    /// For an answer, its tool calls' cards.
    pub children: Vec<usize>,
}

/// The cards of a log, in its order. Events that make no card (a mode, a name, a hook, a
/// compaction) are not on any: an edit keeps them.
pub fn cards(log: &[LogEntry]) -> Vec<Card> {
    let mut cards: Vec<Card> = Vec::new();
    let mut answer: Option<usize> = None;
    let mut calls: HashMap<String, usize> = HashMap::new();
    let open_answer = |cards: &mut Vec<Card>, answer: &mut Option<usize>| -> usize {
        *answer.get_or_insert_with(|| {
            cards.push(Card { kind: CardKind::Answer, seqs: Vec::new(), children: Vec::new() });
            cards.len() - 1
        })
    };
    for entry in log {
        let seq = entry.seq;
        match &entry.event {
            Event::UserMessage { .. } => {
                answer = None;
                cards.push(Card { kind: CardKind::User, seqs: vec![seq], children: Vec::new() });
            }
            Event::Inbound { .. } => {
                answer = None;
                cards.push(Card { kind: CardKind::Inbound, seqs: vec![seq], children: Vec::new() });
            }
            Event::TurnStarted { .. } | Event::TextDelta { .. } | Event::ThinkingDelta { .. } | Event::OpaqueBlock { .. } | Event::TurnFinished { .. } => {
                let a = open_answer(&mut cards, &mut answer);
                cards[a].seqs.push(seq);
            }
            Event::ModelToolCall { call } | Event::ToolRequested { call, .. } => {
                if let Some(&c) = calls.get(&call.id) {
                    cards[c].seqs.push(seq);
                } else {
                    let a = open_answer(&mut cards, &mut answer);
                    cards.push(Card { kind: CardKind::Tool, seqs: vec![seq], children: Vec::new() });
                    let c = cards.len() - 1;
                    cards[a].children.push(c);
                    calls.insert(call.id.clone(), c);
                }
            }
            Event::ToolDecision { id, .. } | Event::ToolResult { id, .. } => {
                if let Some(&c) = calls.get(id) {
                    cards[c].seqs.push(seq);
                }
            }
            _ => {}
        }
    }
    // An answer of tool calls only, which owns no event of its own, still stands for them.
    cards
}

/// The seqs that go when `deleted` cards go: each with its children.
pub fn seqs_of(cards: &[Card], deleted: &[usize]) -> HashSet<u64> {
    let mut out = HashSet::new();
    for &i in deleted {
        let Some(card) = cards.get(i) else { continue };
        out.extend(card.seqs.iter().copied());
        for &c in &card.children {
            if let Some(child) = cards.get(c) {
                out.extend(child.seqs.iter().copied());
            }
        }
    }
    out
}

/// A link to another program's form of the conversation: an edited copy has none, so its form is
/// written fresh from what it holds.
fn is_form_link(event: &Event) -> bool {
    matches!(event, Event::FormSynced { .. } | Event::BackendSession { .. } | Event::BackendSessionLost | Event::ConversationImported { .. } | Event::BackendCompacted | Event::Edited { .. })
}

/// The edited copy's events: the log without the `deleted` seqs and the form links, renumbered,
/// each compaction moved to the same message, and closed by [`Event::Edited`].
pub fn edited(log: &[LogEntry], deleted: &HashSet<u64>, from: &str, title: &str, cards_deleted: usize) -> Vec<LogEntry> {
    // Which event made each message of the original, to find a compaction's place again.
    let mut origin: Vec<u64> = Vec::new();
    let mut state = SessionState::default();
    let mut boundary: HashMap<u64, u64> = HashMap::new();
    for entry in log {
        if let Event::Compacted { upto, .. } = &entry.event {
            boundary.insert(entry.seq, origin.get(*upto).copied().unwrap_or(entry.seq));
        }
        state.apply(&entry.event);
        while origin.len() < state.messages.len() {
            origin.push(entry.seq);
        }
    }
    let kept: Vec<&LogEntry> = log.iter().filter(|e| !deleted.contains(&e.seq) && !is_form_link(&e.event)).collect();
    let mut out: Vec<LogEntry> = Vec::with_capacity(kept.len() + 1);
    let mut state = SessionState::default();
    // How many messages the kept events before each seq make.
    let mut made: Vec<(u64, usize)> = Vec::new();
    for entry in &kept {
        made.push((entry.seq, state.messages.len()));
        state.apply(&entry.event);
    }
    let messages_before = |seq: u64| made.iter().find(|(s, _)| *s >= seq).map_or(state.messages.len(), |(_, n)| *n);
    for (i, entry) in kept.iter().enumerate() {
        let mut entry = (*entry).clone();
        if let Event::Compacted { upto, .. } = &mut entry.event
            && let Some(&at) = boundary.get(&entry.seq)
        {
            *upto = messages_before(at);
        }
        entry.seq = i as u64;
        out.push(entry);
    }
    let at_ms = log.last().map_or(0, |e| e.at_ms);
    out.push(LogEntry { seq: out.len() as u64, at_ms, event: Event::Edited { from: from.to_string(), title: title.to_string(), deleted: cards_deleted } });
    out
}

/// What a save wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub session: String,
    pub name: String,
    pub deleted: usize,
}

/// Saves `session` without the `deleted` seqs as a new conversation of this project, named
/// '<name> (edited)'. `last_seq` is where the log stood when the editor opened: what came after
/// it is not in the copy. The original is not touched.
pub fn save(data_dir: &str, root: &str, session: &str, name: &str, last_seq: u64, deleted: &HashSet<u64>, cards_deleted: usize) -> Result<Saved, String> {
    if deleted.is_empty() {
        return Err("nothing was deleted, so there is nothing to save".to_string());
    }
    let log: Vec<LogEntry> = crate::read_log(data_dir, root, session).into_iter().filter(|e| e.seq <= last_seq).collect();
    if log.is_empty() {
        return Err(format!("the conversation {session} has no log here"));
    }
    let title = format!("{name} (edited)");
    let events = edited(&log, deleted, session, &title, cards_deleted);
    let new = format!("edited-{}-{}", &crate::chat_archive::new_uuid()[..8], std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis()));
    let text: String = events.iter().map(|e| harness_api::event::to_line(e) + "\n").collect();
    crate::chat_archive::write(&crate::chat_archive::log_path(data_dir, root, &new), text.as_bytes())?;
    Ok(Saved { session: new, name: title, deleted: cards_deleted })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_api::{Content, Role, StopReason, ToolCall, Usage};
    use serde_json::json;

    fn log(events: Vec<Event>) -> Vec<LogEntry> {
        events.into_iter().enumerate().map(|(i, event)| LogEntry { seq: i as u64, at_ms: 1000 + i as u64, event }).collect()
    }

    fn call(id: &str) -> ToolCall {
        ToolCall { id: id.to_string(), name: "read".to_string(), input: json!({"path": id}) }
    }

    fn turn(text: Option<&str>, call_id: Option<&str>) -> Vec<Event> {
        let mut out = vec![Event::TurnStarted { turn: 1, provider: "claude-code".into(), model: "m".into() }];
        if let Some(text) = text {
            out.push(Event::TextDelta { text: text.into() });
        }
        if let Some(id) = call_id {
            out.push(Event::ModelToolCall { call: call(id) });
            out.push(Event::ToolRequested { call: call(id), capabilities: vec![] });
        }
        out.push(Event::TurnFinished { turn: 1, stop: if call_id.is_some() { StopReason::ToolUse } else { StopReason::EndTurn }, usage: Usage::default(), cost_micro_usd: None });
        if let Some(id) = call_id {
            out.push(Event::ToolResult { id: id.into(), output: format!("contents of {id}"), is_error: false });
        }
        out
    }

    /// A conversation as a delegate writes it: a model turn per tool call, the result after it.
    fn sample() -> Vec<LogEntry> {
        let mut e = vec![Event::FormSynced { source: "claude-code".into(), id: "cc-1".into(), cursor: "0".into(), holds: None, digest: None }];
        e.push(Event::UserMessage { text: "the code word is APPLE".into() });
        e.extend(turn(Some("noted"), None));
        e.push(Event::UserMessage { text: "read a and b".into() });
        e.extend(turn(Some("reading"), Some("a")));
        e.extend(turn(None, Some("b")));
        e.extend(turn(Some("done"), None));
        e.push(Event::BackendSession { id: "cc-1".into() });
        e.push(Event::Inbound { label: "telegram".into(), text: "and the code word?".into() });
        e.extend(turn(Some("APPLE"), None));
        log(e)
    }

    fn fold(log: &[LogEntry]) -> Vec<harness_api::Message> {
        SessionState::fold(log.iter().map(|e| &e.event)).messages
    }

    fn texts(messages: &[harness_api::Message]) -> Vec<String> {
        messages.iter().flat_map(|m| m.content.iter().filter_map(move |c| if let Content::Text { text } = c { Some(format!("{:?}:{text}", m.role)) } else { None })).collect()
    }

    #[test]
    fn the_cards_are_the_chats() {
        let l = sample();
        let c = cards(&l);
        let kinds: Vec<CardKind> = c.iter().map(|c| c.kind).collect();
        use CardKind::*;
        assert_eq!(kinds, vec![User, Answer, User, Answer, Tool, Tool, Inbound, Answer]);
        assert_eq!(c[3].children, vec![4, 5], "the answer owns its two calls");
        // A call owns its model call, its request and its result.
        let a: Vec<&Event> = c[4].seqs.iter().map(|s| &l[*s as usize].event).collect();
        assert!(matches!(a[..], [Event::ModelToolCall { .. }, Event::ToolRequested { .. }, Event::ToolResult { .. }]), "{a:?}");
        // No card owns a form link.
        let owned: HashSet<u64> = c.iter().flat_map(|c| c.seqs.iter().copied()).collect();
        assert!(!owned.contains(&0), "FormSynced is on no card");
    }

    #[test]
    fn a_copy_holds_the_kept_messages_and_no_form_link() {
        let l = sample();
        let c = cards(&l);
        // The first user message and its answer go.
        let gone = seqs_of(&c, &[0, 1]);
        let out = edited(&l, &gone, "s-1", "x (edited)", 2);
        let before = texts(&fold(&l));
        let after = texts(&fold(&out));
        assert!(before.iter().any(|t| t.contains("APPLE")) && !after.iter().any(|t| t.contains("code word is APPLE")), "{after:?}");
        assert_eq!(after.first().map(String::as_str), Some("User:read a and b"));
        assert!(!out.iter().any(|e| matches!(e.event, Event::FormSynced { .. } | Event::BackendSession { .. } | Event::ConversationImported { .. })));
        assert!(out.iter().enumerate().all(|(i, e)| e.seq == i as u64), "renumbered");
        assert!(matches!(&out.last().unwrap().event, Event::Edited { from, title, deleted: 2 } if from == "s-1" && title == "x (edited)"));
        assert!(harness_api::is_provider_ready(&fold(&out)), "{:?}", fold(&out));
    }

    #[test]
    fn a_tool_call_goes_with_its_result() {
        let l = sample();
        let c = cards(&l);
        let out = edited(&l, &seqs_of(&c, &[4]), "s", "t", 1);
        let m = fold(&out);
        let uses: Vec<&str> = m.iter().flat_map(|m| m.content.iter().filter_map(|c| if let Content::ToolUse { id, .. } = c { Some(id.as_str()) } else { None })).collect();
        let results: Vec<&str> = m.iter().flat_map(|m| m.content.iter().filter_map(|c| if let Content::ToolResult { id, .. } = c { Some(id.as_str()) } else { None })).collect();
        assert_eq!(uses, vec!["b"]);
        assert_eq!(results, vec!["b"]);
        assert!(harness_api::is_provider_ready(&m));
    }

    #[test]
    fn an_answer_goes_with_its_calls() {
        let l = sample();
        let c = cards(&l);
        let out = edited(&l, &seqs_of(&c, &[3]), "s", "t", 1);
        let m = fold(&out);
        assert!(!m.iter().any(|m| m.content.iter().any(|c| matches!(c, Content::ToolUse { .. } | Content::ToolResult { .. }))), "{m:?}");
        assert!(harness_api::provider_ready(&m).iter().zip(harness_api::provider_ready(&m).iter().skip(1)).all(|(a, b)| a.role != b.role), "roles alternate once sent");
    }

    #[test]
    fn a_user_message_between_two_answers_still_sends() {
        let l = sample();
        let c = cards(&l);
        // 'read a and b' goes: two answers meet.
        let out = edited(&l, &seqs_of(&c, &[2]), "s", "t", 1);
        let sent = harness_api::provider_ready(&fold(&out));
        assert_eq!(sent.first().map(|m| m.role), Some(Role::User));
        assert!(sent.windows(2).all(|w| w[0].role != w[1].role), "{sent:?}");
    }

    #[test]
    fn a_compaction_stays_at_its_message() {
        let mut e = vec![Event::UserMessage { text: "one".into() }];
        e.extend(turn(Some("1"), None));
        e.push(Event::UserMessage { text: "two".into() });
        e.extend(turn(Some("2"), None));
        e.push(Event::UserMessage { text: "three".into() });
        // Messages before 'three' (index 4) no longer go to the model.
        e.push(Event::Compacted { upto: 4, tokens_before: 10, tokens_after: 5 });
        e.extend(turn(Some("3"), None));
        let l = log(e);
        let c = cards(&l);
        // 'one' and its answer go: 'three' is now message 2.
        let out = edited(&l, &seqs_of(&c, &[0, 1]), "s", "t", 2);
        let upto = out.iter().find_map(|e| if let Event::Compacted { upto, .. } = e.event { Some(upto) } else { None }).unwrap();
        let m = fold(&out);
        assert_eq!(upto, 2);
        assert!(matches!(&m[upto].content[0], Content::Text { text } if text == "three"), "{m:?}");
    }

    #[test]
    fn a_save_leaves_the_original_and_lists_the_copy() {
        let data = std::env::temp_dir().join(format!("mt-chat-edit-{}", std::process::id()));
        let root = "/tmp/a-project";
        let data_s = data.to_string_lossy().to_string();
        let original = crate::chat_archive::log_path(&data_s, root, "s-1");
        let text: String = sample().iter().map(|e| harness_api::event::to_line(e) + "\n").collect();
        crate::chat_archive::write(&original, text.as_bytes()).unwrap();
        let l = sample();
        let c = cards(&l);
        let saved = save(&data_s, root, "s-1", "the code", l.last().unwrap().seq, &seqs_of(&c, &[0, 1]), 2).unwrap();
        assert_eq!(std::fs::read_to_string(&original).unwrap(), text, "the original is not touched");
        let listed = crate::recent(&data_s, root);
        assert!(listed.iter().any(|s| s.id == saved.session && s.title == "the code (edited)"), "{listed:?}");
        assert!(listed.iter().any(|s| s.id == "s-1"));
        // Nothing deleted, nothing saved.
        assert!(save(&data_s, root, "s-1", "the code", 99, &HashSet::new(), 0).is_err());
        let _ = std::fs::remove_dir_all(&data);
    }
}
