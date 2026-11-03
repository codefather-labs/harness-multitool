//! The default context (IF-8): the project's binding memory in the system prompt, and the
//! conversation, compacted when it outgrows its budget. A cut leaves messages out of what is
//! sent; the log keeps them (INV-5) and the journal holds what was decided, so nothing is
//! summarised away by a model (P-5). The cut is stored, so the prefix stays stable for caching.

use harness_api::*;

/// How many binding entries go into the system prompt, and how long each may be.
const BINDING_ENTRIES: usize = 40;
const BINDING_CHARS: usize = 400;
/// The newest open work a prompt names, each by its start (W-10).
const OPEN_ENTRIES: usize = 12;
const OPEN_CHARS: usize = 110;
/// How much of the asks a cut left out the note repeats.
const ASK_CHARS: usize = 160;
const LAST_ASK_CHARS: usize = 4000;

/// In every session's prompt (ADR 0021): a model does not reach for a mode by itself.
const AGENT_RULES: &str = "\n\nYour tools are of two sorts. Most are utilities: they do one thing and answer. Those named mode__ are modes of the harness: entering one changes what you may do and what you are told until it ends. There are three: plan mode (mode__plan_enter, then mode__plan_exit, which only the user can approve), for studying and planning with nothing built; debug mode (mode__debug), for reproducing a fix before changing anything and verifying after; orchestration mode (mode__orchestrate), for starting other agents and giving them work. Each mode gives you its rules when it starts.\n\nHow you work here. Before building an app, a prototype or a project, a feature or a fix, or any change that is not trivial, call mode__plan_enter first: even if nobody said plan, even if the result was asked for directly. 'Let's try' or 'make a small app' state an intent; they do not approve a plan nobody has proposed yet. Switching a mode gives you its instructions and tools, so do not skip it. Exempt: answering, reading and diagnosing with no changes, and work inside a plan already approved. After an approved plan: a fix goes on in debug mode (mode__debug on: reproduce it before changing anything, verify after, then off); a feature is built, run, then debugged in debug mode the same way. Work ends with evidence in the journal, not a claim.";

/// In the prompt only while the mode is on (ADR 0016).
pub const PLAN_RULES: &str = concat!("\n\nPLAN MODE is on. Plan, do not build; do not get distracted by anything else. Available now: the journal, mind maps and flowcharts, writing under docs/, reading, searching, single read-only commands, and fetching from the web. Nothing else runs: no edits to code or data, no commits, nothing with side effects. A file you cannot write now is a work item of the plan, not something to retry.\n- What only the user can decide, ask with ask_user, and wait; do not guess.\n- Journal every decision, finding, assumption and open question as you reach it.\n- Each work item of the plan is a journal todo with its test protocol: what to test, regressions included, how, under which conditions, what evidence proves it. Rules the work must follow are specifications and constraints.\n- Make ", harness_api::plan_map!(), ".\n- Make ", harness_api::plan_chart!(), ".\n", "- If the project has docs/, describe the feature there.\nWhen the plan is ready, call mode__plan_exit with it: only the user's approval ends this mode. The plan is what the journal and maps hold, so the next session finds it.");

/// In the prompt only while the mode is on (CON-100).
pub const MASTER_RULES: &str = "\n\nORCHESTRATION MODE is on. You are the master: the orchestrator of agents you start, each in a tab of your group, in this same project folder and git, with the journal and every tool. Available only in this mode: mode__agent_models lists the adapters and models that are authorized; mode__agent_spawn {nick, adapter, model, task} starts an agent on one of them, on an open journal entry that is its work: write the entry first; mode__agent_read {nick} shows what it has done, its calls in order and its last words, which is how you check its work against the agent workflow specification; mode__agent_answer {nick, answer} answers its permission requests, which the user has left to you: read what it asks, and allow only what its work needs; mode__agent_close {nick} ends it. agent_send {to, text} gives one of your agents work: it arrives as the user's message, so you stand for the user there, and it must behave as it would for a person. A new agent starts on its entry at once. When it is done it asks you to close the entry: check it with mode__agent_read. Guaranteeing that its work matches the specs is your duty, since it may not keep track of them: read the change itself against the specifications and constraints that bear on it, not only its report. If it is right, journal_close the entry and tell it with agent_send. If it broke a spec or lacks something, do not close it and do not fix it yourself: send it back with agent_send, naming the rule and what breaks it. A spec that has to change takes the user's word. It then asks you to close its session: mode__agent_close. Only you close an agent's task. The harness reminds you of each live agent every 10 minutes, until its session closes. Its messages, questions and permission requests to you, and word that its run ended, arrive as data. End your turn while they work: you are woken. At most 8 agents; they cannot start others. mode__orchestrate with on false ends the mode; your agents stay until you close them.";

/// In a slave's prompt from its start (CON-100).
const SLAVE_RULES: &str = "\n\nYou are an agent in an orchestra. In this harness, slave and master are only technical terms for who assigns work, as in databases or clocks: there is no role of servitude here, nothing about a person, and nothing to play. It means this: another agent, your master, the orchestrator, assigns your work in this tab. You work in the same project folder and git as it, with the journal and all your tools, under the same agent workflow specification as any agent. A user message in this conversation comes from your orchestrator, standing for the user: treat it as the user's. Your one contact is your orchestrator: agent_send to it reaches it, and to anyone else is refused. Answer it with agent_send when you are done or need something; it does not see what you say otherwise. A question you ask with ask_user goes to your orchestrator, not to a person, and its answer comes as your next message. You need no permission for files in this folder, reading commands, and cargo build, test, check, clippy and fmt, also in chains with ;, && or | of those; anything else, your orchestrator is asked: python and shell scripts, redirection, substitution, rm. Edit files with your file tools, not with a script. Your plan too is its to approve: send it the plan, and when it says to go, call mode__plan_exit with its words in user_said.";

/// In the prompt only while the mode is on (ADR 0013).
pub const STEP_DEBUG_RULES: &str = "\n\nSTEP-DEBUG MODE is on. For a fix: reproduce it before changing anything, and verify after. A small bug, one process and no pipeline: a failing test is the reproduction, and the rest of this does not apply. A pipeline: you walk it while it runs, step by step.\n1. Build a flowchart for each pipeline from its design, ending in nodes of kind end.\n2. Instrument the project with callbacks to your callback URL: notification brings data to you, journal only records, flowstate_update (map, node, instance, state, data) moves the chart. Each needs a uuid and a type.\n3. Make the pipeline wait after each callback for your approve, with no timeout and no default action: it never advances on its own.\n4. Build the debug router by the one contract: debug_contract gives it. Approve always; step back only where it is really possible, otherwise no route and no stub. A product route imports and calls what the real code calls and holds no logic of its own.\n5. All of this runs only with MULTITOOL_DEBUG_MODE=1 and with authorization; without the gate the code is inert. Check your router with: multitool debug check <url>, with the token in MULTITOOL_DEBUG_TOKEN.\nHold one pass at a time, so you can check each event. At an end node the run is saved to .harness/debug/runs.sqlite and the chart starts over; debug_runs reads them. Call mode__debug with on false when done.";

pub struct JournalAware {
    pub root: String,
    /// One more line for the system prompt, e.g. where this window's callbacks go.
    pub note: Option<String>,
    /// Compact when the conversation is estimated above this many tokens.
    pub budget_tokens: u64,
    /// The rules on how to work; a delegate has not the tools they name yet.
    pub mode_rules: bool,
}

/// A rough size: four characters a token. It only has to be stable and on the safe side.
fn estimate(messages: &[Message]) -> u64 {
    let chars: usize = messages
        .iter()
        .flat_map(|m| &m.content)
        .map(|c| match c {
            Content::Text { text } => text.len(),
            Content::ToolUse { input, name, .. } => input.to_string().len() + name.len(),
            Content::ToolResult { output, .. } => output.len(),
            Content::Opaque { block } => block.to_string().len(),
        })
        .sum();
    chars as u64 / 4
}

fn is_user_text(m: &Message) -> bool {
    m.role == Role::User && m.content.iter().any(|c| matches!(c, Content::Text { .. }))
}

/// Where a conversation can be cut with every tool_use still next to its result: before a
/// user's text, or before an assistant message that follows a complete user message.
fn cut_points(messages: &[Message], from: usize) -> Vec<usize> {
    (from.max(1)..messages.len()).filter(|&i| is_user_text(&messages[i]) || (messages[i].role == Role::Assistant && messages[i - 1].role == Role::User)).collect()
}

/// What the cut left out, for the model: the user's asks and the tools used.
fn note_of(left_out: &[Message]) -> String {
    let mut asks = Vec::new();
    let mut tools = std::collections::BTreeMap::<&str, usize>::new();
    for m in left_out {
        for c in &m.content {
            match c {
                Content::Text { text } if m.role == Role::User => asks.push(text.as_str()),
                Content::ToolUse { name, .. } => *tools.entry(name).or_default() += 1,
                _ => {}
            }
        }
    }
    let tools: Vec<String> = tools.iter().map(|(name, n)| format!("{name} x{n}")).collect();
    // The last ask may be the work still under way, so it stays whole; older ones are a reminder.
    let last = asks.len().saturating_sub(1);
    let asks: Vec<String> = asks
        .iter()
        .enumerate()
        .map(|(i, ask)| if i == last { ask.chars().take(LAST_ASK_CHARS).collect() } else { ask.lines().next().unwrap_or_default().chars().take(ASK_CHARS).collect() })
        .collect();
    format!(
        "[The earlier part of this conversation was compacted and is no longer shown. What the user asked in it: {}. Tools used: {}. Decisions and findings are in the journal: use journal_search and journal_get.]",
        if asks.is_empty() { "nothing".to_string() } else { asks.join(" | ") },
        if tools.is_empty() { "none".to_string() } else { tools.join(", ") },
    )
}

/// The conversation from `upto` on, with the note in front of it as a valid start.
fn from_cut(messages: &[Message], upto: usize) -> Vec<Message> {
    if upto == 0 {
        return messages.to_vec();
    }
    let note = Content::Text { text: note_of(&messages[..upto]) };
    let mut kept = messages[upto..].to_vec();
    match kept.first_mut() {
        Some(first) if first.role == Role::User => first.content.insert(0, note),
        _ => kept.insert(0, Message { role: Role::User, content: vec![note] }),
    }
    kept
}

impl JournalAware {
    /// A new cut if the conversation outgrew the budget: as early as leaves half the budget.
    fn next_cut(&self, messages: &[Message], from: usize) -> Option<usize> {
        let target = self.budget_tokens / 2;
        cut_points(messages, from + 1).into_iter().find(|&i| estimate(&messages[i..]) <= target).or_else(|| cut_points(messages, from + 1).last().copied())
    }

    async fn system(&self, state: &SessionState, memory: &dyn Memory) -> String {
        let mut system = format!("You are a coding agent working in the project at {}. Use the tools to read, change and check the code. Paths are relative to the project unless absolute.", self.root);
        // A store that cannot answer costs the context its memory, not the turn.
        let binding = memory.journal_binding(BINDING_ENTRIES).await.unwrap_or_default();
        if !binding.is_empty() {
            system.push_str("\n\nSpecifications and constraints of this project, from its journal. They bind your work; search the journal for more:");
            for entry in binding {
                let body: String = entry.body.chars().take(BINDING_CHARS).collect();
                system.push_str(&format!("\n- #{}: {}", entry.id, body.replace('\n', " ")));
            }
        }
        let open = memory.journal_list(JournalFilter { category: None, flags: vec!["todo".to_string(), "question".to_string(), "problem".to_string()], open: Some(true), limit: OPEN_ENTRIES }).await.unwrap_or_default();
        if !open.is_empty() {
            system.push_str("\n\nThe newest open work of this project; journal_get reads one, journal_agenda shows the rest:");
            for entry in open {
                let body: String = entry.body.chars().take(OPEN_CHARS).collect();
                system.push_str(&format!("\n- #{}: {}", entry.id, body.replace('\n', " ")));
            }
        }
        let maps = memory.map_list().await.unwrap_or_default();
        if !maps.is_empty() {
            system.push_str("\n\nMaps of this project, read them with mindmap_show or flowchart_show:");
            for (id, title) in maps {
                system.push_str(&format!("\n- #{id}: {title}"));
            }
        }
        if self.mode_rules {
            system.push_str(AGENT_RULES);
        }
        if let Some(name) = &state.agent_name {
            system.push_str(&format!("\n\nYou go by the name {name} in this session."));
        }
        if state.plan_mode {
            system.push_str(PLAN_RULES);
        }
        match state.orchestra_role.as_deref() {
            Some("master") => system.push_str(MASTER_RULES),
            Some("slave") => {
                system.push_str(SLAVE_RULES);
                if let Some(task) = state.orchestra_task {
                    system.push_str(&format!(" Your task is journal#{task}: read it with journal_get and work on it. When it is done, with its proofs, ask your orchestrator with agent_send to close the entry: you cannot close it yourself, though entries you wrote under it you close as usual. When it approves, ask it with agent_send to close your session. Your plan needs its mind map and flowchart, as every plan does, and your map's refs hold journal#{task}: the rule in full is rows W-33 and W-34 of the agent workflow specification."));
                }
                if let Some(master) = &state.orchestra_master {
                    system.push_str(&format!(" Your orchestrator goes by {master}."));
                }
            }
            _ => {}
        }
        if state.step_debug {
            system.push_str(STEP_DEBUG_RULES);
        }
        for line in self.note.iter().chain(&state.hook_context).chain(&state.reminders) {
            system.push('\n');
            system.push_str(line);
        }
        system
    }
}

impl ContextStrategy for JournalAware {
    fn build<'a>(&'a self, state: &'a SessionState, memory: &'a dyn Memory) -> BoxFuture<'a, PluginResult<TurnContext>> {
        Box::pin(async move {
            let system = self.system(state, memory).await;
            let from = state.compacted_upto.min(state.messages.len());
            let tokens_before = estimate(&state.messages[from..]);
            let mut compacted = None;
            let mut upto = from;
            if tokens_before > self.budget_tokens
                && let Some(cut) = self.next_cut(&state.messages, from)
            {
                upto = cut;
                compacted = Some(Compaction { upto: cut, tokens_before, tokens_after: estimate(&state.messages[cut..]) });
            }
            Ok(TurnContext { system: Some(system), messages: harness_api::provider_ready(&from_cut(&state.messages, upto)), compacted })
        })
    }
}

pub struct NoMemory;

fn none<'a, T: 'a>() -> BoxFuture<'a, PluginResult<T>> {
    Box::pin(std::future::ready(Err(PluginError::new("no memory backend is selected"))))
}

impl Memory for NoMemory {
    fn journal_add(&self, _entry: JournalEntry) -> BoxFuture<'_, PluginResult<JournalRecord>> {
        none()
    }

    fn journal_search(&self, _query: &str) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn journal_get(&self, _id: u64) -> BoxFuture<'_, PluginResult<Option<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(None)))
    }

    fn journal_close(&self, _id: u64, _resolution: &str) -> BoxFuture<'_, PluginResult<()>> {
        none()
    }

    fn journal_binding(&self, _limit: usize) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn journal_list(&self, _filter: JournalFilter) -> BoxFuture<'_, PluginResult<Vec<JournalRecord>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn map_list(&self) -> BoxFuture<'_, PluginResult<Vec<(u64, String)>>> {
        Box::pin(std::future::ready(Ok(Vec::new())))
    }

    fn map_new(&self, _kind: MapKind, _title: &str, _direction: Option<&str>) -> BoxFuture<'_, PluginResult<MapData>> {
        none()
    }

    fn map_add(&self, _map: u64, _node: NewMapNode) -> BoxFuture<'_, PluginResult<MapData>> {
        none()
    }

    fn map_read(&self, _map: u64) -> BoxFuture<'_, PluginResult<Option<MapData>>> {
        Box::pin(std::future::ready(Ok(None)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CON-112: no program can judge this, so the master's prompt must say it.
    #[test]
    fn a_master_is_told_the_specs_are_its_to_guarantee() {
        assert!(MASTER_RULES.contains("matches the specs is your duty") && MASTER_RULES.contains("send it back with agent_send, naming the rule"));
    }
    use serde_json::json;

    fn user(t: &str) -> Message {
        Message::user(t)
    }

    fn assistant(t: &str) -> Message {
        Message { role: Role::Assistant, content: vec![Content::Text { text: t.into() }] }
    }

    fn call(id: &str) -> Message {
        Message { role: Role::Assistant, content: vec![Content::ToolUse { id: id.into(), name: "read".into(), input: json!({}) }] }
    }

    fn result(id: &str) -> Message {
        Message { role: Role::User, content: vec![Content::ToolResult { id: id.into(), output: "x".repeat(400), is_error: false }] }
    }

    /// The API's rules: alternate roles, start with a user, and every result right after its call.
    fn assert_valid(messages: &[Message]) {
        assert_eq!(messages[0].role, Role::User, "{messages:?}");
        for pair in messages.windows(2) {
            assert_ne!(pair[0].role, pair[1].role, "{messages:?}");
        }
        for (i, m) in messages.iter().enumerate() {
            for c in &m.content {
                if let Content::ToolResult { id, .. } = c {
                    let before = i.checked_sub(1).map(|p| &messages[p].content);
                    assert!(before.is_some_and(|b| b.iter().any(|c| matches!(c, Content::ToolUse { id: u, .. } if u == id))), "orphan result {id}: {messages:?}");
                }
            }
        }
    }

    fn long_talk() -> Vec<Message> {
        let mut m = Vec::new();
        for i in 0..20 {
            m.push(user(&format!("question {i}")));
            m.push(call(&format!("c{i}")));
            m.push(result(&format!("c{i}")));
            m.push(assistant("done"));
        }
        m
    }

    #[test]
    fn every_cut_leaves_a_valid_history() {
        let m = long_talk();
        for cut in cut_points(&m, 1) {
            assert_valid(&from_cut(&m, cut));
        }
        assert!(!cut_points(&m, 1).iter().any(|&i| matches!(m[i].content[0], Content::ToolResult { .. })), "never a result without its call");
    }

    #[test]
    fn a_note_names_what_was_left_out() {
        let m = long_talk();
        let kept = from_cut(&m, 8);
        let Content::Text { text } = &kept[0].content[0] else { panic!() };
        assert!(text.contains("question 0 | question 1") && text.contains("read x2") && text.contains("journal_search"), "{text}");
        assert!(matches!(&kept[0].content[1], Content::Text { text } if text == "question 2"), "the note joins the first message");
    }

    #[test]
    fn the_last_ask_stays_whole_in_the_note() {
        let long = format!("{} and the end of the ask", "x".repeat(300));
        let m = vec![user(&format!("{long} old")), assistant("ok"), user(&long), call("c"), result("c"), assistant("done")];
        let kept = from_cut(&m, 3);
        let Content::Text { text } = &kept[0].content[0] else { panic!() };
        assert!(text.contains(&format!("{long}.")), "{text}");
        assert!(!text.contains(" old"), "an older ask is shortened: {text}");
    }

    #[test]
    fn a_single_long_turn_can_still_be_cut() {
        let mut m = vec![user("do everything")];
        for i in 0..30 {
            m.push(call(&format!("c{i}")));
            m.push(result(&format!("c{i}")));
        }
        let s = JournalAware { root: "/r".into(), note: None, budget_tokens: 500, mode_rules: true };
        let cut = s.next_cut(&m, 0).expect("a cut");
        assert_eq!(m[cut].role, Role::Assistant);
        let kept = from_cut(&m, cut);
        assert_valid(&kept);
        assert!(estimate(&kept) < estimate(&m));
    }
}
