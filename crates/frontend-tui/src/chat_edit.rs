//! The conversation editor of Settings > Chats (CON-189): the conversation drawn as the chat draws
//! it, card by card; cards marked with Enter, deleted with d, brought back with u, and the copy
//! without them saved with s.

use std::collections::BTreeSet;

use harness_api::LogEntry;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::input::Key;
use crate::live::{Feed, TimeFormat};
use crate::model::{Item, Outgoing, Overlay, SessionView, SettingsView, Workspace};
use crate::theme::Theme;
use crate::view::{Drawn, Hit, Target};

/// What a card is, as harness-session's `chat_edit` groups a log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    User,
    Inbound,
    Answer,
    Tool,
}

/// One card of the editor.
#[derive(Debug, Clone)]
pub struct EditCard {
    pub kind: CardKind,
    /// The log events it owns.
    pub seqs: Vec<u64>,
    /// The cards that go with it (an answer's tool calls).
    pub children: Vec<usize>,
    /// What the chat draws for it.
    pub items: Vec<Item>,
}

/// The editor (CON-189).
#[derive(Debug, Clone)]
pub struct ChatEditView {
    pub session: String,
    pub name: String,
    /// Where the log stood when it opened: the copy holds nothing after it.
    pub last_seq: u64,
    pub cards: Vec<EditCard>,
    /// Deleted, and so not drawn.
    pub gone: Vec<bool>,
    pub cursor: usize,
    pub marked: BTreeSet<usize>,
    /// Each deletion, for u.
    pub undo: Vec<Vec<usize>>,
    /// Esc with unsaved deletions: asking whether to leave.
    pub asking_leave: bool,
    pub status: String,
    /// Settings as they were, to go back to.
    pub back: Box<SettingsView>,
}

impl ChatEditView {
    /// The editor over `log`, its cards as `cards` gives them: (kind, owned seqs, children).
    pub fn new(session: String, name: String, log: &[LogEntry], cards: Vec<(CardKind, Vec<u64>, Vec<usize>)>, time: TimeFormat, back: SettingsView) -> Self {
        let by_seq: std::collections::HashMap<u64, &LogEntry> = log.iter().map(|e| (e.seq, e)).collect();
        let cards: Vec<EditCard> = cards
            .into_iter()
            .map(|(kind, seqs, children)| {
                let mut v = SessionView::default();
                let mut feed = Feed::new(time, 200_000);
                for s in &seqs {
                    if let Some(e) = by_seq.get(s) {
                        feed.apply(&mut v, e);
                    }
                }
                let mut items = v.transcript;
                // A tool call alone is drawn as the card itself, not inside an empty answer.
                if kind == CardKind::Tool
                    && let [Item::Turn { lines, attached, .. }] = items.as_slice()
                    && lines.iter().all(|l| l.trim().is_empty())
                {
                    items = attached.clone();
                }
                EditCard { kind, seqs, children, items }
            })
            .collect();
        let gone = vec![false; cards.len()];
        let last_seq = log.last().map_or(0, |e| e.seq);
        let status = if cards.is_empty() { "This conversation has nothing to edit.".to_string() } else { String::new() };
        ChatEditView { session, name, last_seq, cards, gone, cursor: 0, marked: BTreeSet::new(), undo: Vec::new(), asking_leave: false, status, back: Box::new(back) }
    }

    fn deleted(&self) -> usize {
        self.gone.iter().filter(|g| **g).count()
    }

    /// The next card shown from `from` in `step`'s direction, `step` times.
    fn moved(&self, from: usize, step: i64) -> usize {
        let mut at = from;
        let dir = step.signum();
        for _ in 0..step.unsigned_abs() {
            let mut next = at as i64 + dir;
            while next >= 0 && (next as usize) < self.cards.len() && self.gone[next as usize] {
                next += dir;
            }
            if next < 0 || next as usize >= self.cards.len() {
                break;
            }
            at = next as usize;
        }
        at
    }

    /// The cursor on a card shown: where it is, or the nearest one after, or before.
    fn settle(&mut self) {
        if self.cards.is_empty() || !self.gone[self.cursor.min(self.cards.len() - 1)] {
            return;
        }
        let after = (self.cursor..self.cards.len()).find(|&i| !self.gone[i]);
        let before = (0..self.cursor).rev().find(|&i| !self.gone[i]);
        self.cursor = after.or(before).unwrap_or(0);
    }

    /// d: the marked cards go, or the one under the cursor; each with its children.
    fn delete(&mut self) {
        if self.cards.is_empty() || self.gone.iter().all(|g| *g) {
            return;
        }
        let picked: Vec<usize> = if self.marked.is_empty() { vec![self.cursor] } else { self.marked.iter().copied().collect() };
        let mut went = Vec::new();
        for i in picked {
            for j in std::iter::once(i).chain(self.cards[i].children.clone()) {
                if !self.gone[j] {
                    self.gone[j] = true;
                    went.push(j);
                }
            }
        }
        self.marked.clear();
        if !went.is_empty() {
            self.status = format!("{} deleted. u brings {} back; s saves a copy without {}.", plural(went.len()), if went.len() == 1 { "it" } else { "them" }, if went.len() == 1 { "it" } else { "them" });
            self.undo.push(went);
        }
        self.settle();
    }

    fn bring_back(&mut self) {
        if let Some(went) = self.undo.pop() {
            for &j in &went {
                self.gone[j] = false;
            }
            self.status = format!("{} brought back.", plural(went.len()));
            self.cursor = went[0];
        }
    }

    /// The seqs the copy leaves out.
    pub fn deleted_seqs(&self) -> Vec<u64> {
        let mut out: Vec<u64> = self.cards.iter().zip(&self.gone).filter(|(_, g)| **g).flat_map(|(c, _)| c.seqs.iter().copied()).collect();
        out.sort_unstable();
        out
    }
}

fn plural(n: usize) -> String {
    if n == 1 { "1 card".to_string() } else { format!("{n} cards") }
}

/// Back to Settings > Chats, saying `status` there.
fn leave(w: &mut Workspace, status: Option<String>) {
    let Some(Overlay::ChatEdit(e)) = w.overlay.take() else { return };
    let mut back = *e.back;
    if let Some(status) = status {
        back.chats.status = status;
    }
    w.overlay = Some(Overlay::Settings(back));
}

/// A key in the editor.
pub fn key(w: &mut Workspace, k: Key) {
    let Some(Overlay::ChatEdit(e)) = &mut w.overlay else { return };
    if e.asking_leave {
        match k {
            Key::Char('y' | 'Y') | Key::Enter => leave(w, Some("Left the editor; nothing was saved.".to_string())),
            Key::Char('n' | 'N') | Key::Esc => e.asking_leave = false,
            Key::Char('s') => save(w),
            _ => {}
        }
        return;
    }
    match k {
        Key::Up => e.cursor = e.moved(e.cursor, -1),
        Key::Down => e.cursor = e.moved(e.cursor, 1),
        Key::PageUp => e.cursor = e.moved(e.cursor, -10),
        Key::PageDown => e.cursor = e.moved(e.cursor, 10),
        Key::Home => e.cursor = e.moved(e.cursor, -(e.cards.len() as i64)),
        Key::End => e.cursor = e.moved(e.cursor, e.cards.len() as i64),
        Key::Enter | Key::Char(' ') => toggle(e, e.cursor),
        Key::Char('d') | Key::Delete => e.delete(),
        Key::Char('u') => e.bring_back(),
        Key::Char('s') => save(w),
        Key::Esc if e.deleted() > 0 => e.asking_leave = true,
        Key::Esc => leave(w, None),
        _ => {}
    }
}

fn toggle(e: &mut ChatEditView, i: usize) {
    if i >= e.cards.len() || e.gone[i] {
        return;
    }
    e.cursor = i;
    if !e.marked.remove(&i) {
        e.marked.insert(i);
    }
}

/// s: the copy without the deleted cards is saved; the editor goes back to the list.
fn save(w: &mut Workspace) {
    let Some(Overlay::ChatEdit(e)) = &mut w.overlay else { return };
    let cards = e.deleted();
    if cards == 0 {
        e.status = "Nothing is deleted yet: mark cards with Enter and delete them with d.".to_string();
        e.asking_leave = false;
        return;
    }
    w.outbox.push(Outgoing::ChatEditSave { session: e.session.clone(), name: e.name.clone(), last_seq: e.last_seq, deleted: e.deleted_seqs(), cards });
    let said = format!("Saving \"{} (edited)\" without {}...", e.name, plural(cards));
    leave(w, Some(said));
}

/// A click on a card marks it.
pub fn click(w: &mut Workspace, i: usize) {
    if let Some(Overlay::ChatEdit(e)) = &mut w.overlay
        && !e.asking_leave
    {
        toggle(e, i);
    }
}

/// The wheel moves the cursor: `rows` positive is back in time.
pub fn wheel(e: &mut ChatEditView, rows: i32) {
    let steps = (rows.abs() / 3).max(1) as i64;
    e.cursor = e.moved(e.cursor, if rows > 0 { -steps } else { steps });
}

const GUTTER: u16 = 3;

fn line(f: &mut Frame, x: u16, y: u16, w: u16, spans: Vec<Span<'static>>, bg: Color) {
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)), Rect::new(x, y, w, 1));
}

/// A card's drawing: its items stacked, one row between them, or one line when it has none.
fn card_rows(c: &EditCard, width: u16, t: &Theme) -> Vec<crate::transcript::Drawn> {
    c.items
        .iter()
        .map(|it| crate::transcript::guarded(|| crate::transcript::item(it, width, t, &[])))
        .collect::<Option<Vec<_>>>()
        .unwrap_or_default()
}

fn card_height(rows: &[crate::transcript::Drawn]) -> u16 {
    if rows.is_empty() { 1 } else { rows.iter().map(|d| d.buf.area.height).sum::<u16>() + rows.len() as u16 - 1 }
}

/// The editor, filling the window.
pub fn draw(f: &mut Frame, area: Rect, e: &ChatEditView, t: &Theme, out: &mut Drawn) {
    let bg = t.surface;
    {
        let buf = f.buffer_mut();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                buf[(x, y)].reset();
                buf[(x, y)].set_bg(bg);
            }
        }
    }
    if area.height < 6 || area.width < 20 {
        return;
    }
    let muted = Style::default().fg(t.text_muted);
    let accent = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
    let title = format!(" Edit · {}", e.name);
    let counts = format!("{} deleted · {} marked ", e.deleted(), e.marked.len());
    line(f, area.x, area.y, area.width, vec![Span::styled(title, accent)], bg);
    let cw = counts.chars().count() as u16;
    line(f, area.x + area.width.saturating_sub(cw), area.y, cw, vec![Span::styled(counts, muted)], bg);
    let help = if e.asking_leave {
        "Leave without saving? y leaves · n stays · s saves".to_string()
    } else {
        "Up/Down move · Enter marks · d deletes · u brings back · s saves a copy · Esc leaves".to_string()
    };
    let foot = area.y + area.height - 1;
    line(f, area.x, foot, area.width, vec![Span::styled(format!(" {help}"), if e.asking_leave { accent } else { muted })], bg);
    if !e.status.is_empty() {
        line(f, area.x, foot - 1, area.width, vec![Span::styled(format!(" {}", e.status), Style::default().fg(t.text))], bg);
    }
    let body = Rect::new(area.x, area.y + 2, area.width, area.height.saturating_sub(5));
    let width = body.width.saturating_sub(GUTTER + 1);
    if e.cards.iter().zip(&e.gone).all(|(_, g)| *g) {
        line(f, body.x + 1, body.y, body.width - 1, vec![Span::styled(if e.cards.is_empty() { "Nothing here to edit." } else { "Every card is deleted: s saves an empty copy, u brings them back." }, muted)], bg);
        return;
    }
    // The cursor's card a third of the way down, the cards before it above.
    let mut top_y = body.height / 3;
    let mut first = e.cursor;
    while top_y > 0 {
        let Some(prev) = (0..first).rev().find(|&i| !e.gone[i]) else { break };
        let h = card_height(&card_rows(&e.cards[prev], width, t)) + 1;
        if h > top_y {
            break;
        }
        top_y -= h;
        first = prev;
    }
    if first == e.cursor || (0..first).all(|i| e.gone[i]) {
        top_y = 0;
    }
    let mut y = body.y + top_y;
    let bottom = body.y + body.height;
    for i in first..e.cards.len() {
        if y >= bottom {
            break;
        }
        if e.gone[i] {
            continue;
        }
        let c = &e.cards[i];
        let rows = card_rows(c, width, t);
        let h = card_height(&rows);
        let under = i == e.cursor;
        let marked = e.marked.contains(&i);
        let mark_style = Style::default().fg(if marked { t.accent } else { t.accent_line }).add_modifier(Modifier::BOLD);
        // The gutter: the cursor, and a bar down a marked card.
        for r in 0..h {
            let ry = y + r;
            if ry >= bottom {
                break;
            }
            let sym = match (under && r == 0, marked) {
                (true, true) => "▶●",
                (true, false) => "▶ ",
                (false, true) => " ●",
                _ => "  ",
            };
            let sym = if r > 0 && marked { " ┃" } else { sym };
            line(f, body.x, ry, GUTTER, vec![Span::styled(sym, mark_style)], if marked { t.surface_raised } else { bg });
        }
        if rows.is_empty() {
            let calls = c.children.iter().filter(|&&j| !e.gone[j]).count();
            let words = match c.kind {
                CardKind::Answer => format!("the agent: {calls} tool call{} below", if calls == 1 { "" } else { "s" }),
                _ => "(nothing drawn)".to_string(),
            };
            line(f, body.x + GUTTER, y, width, vec![Span::styled(words, muted)], if marked { t.surface_raised } else { bg });
        } else {
            let mut ry = y;
            for d in rows {
                for row in 0..d.buf.area.height {
                    let sy = ry + row;
                    if sy >= bottom {
                        break;
                    }
                    for col in 0..d.buf.area.width.min(width) {
                        let mut cell = d.buf[(col, row)].clone();
                        if marked {
                            cell.set_bg(crate::transcript::tint(t.accent, cell.bg, 0.12));
                        }
                        f.buffer_mut()[(body.x + GUTTER + col, sy)] = cell;
                    }
                }
                ry += d.buf.area.height + 1;
            }
        }
        let shown = h.min(bottom - y);
        out.hits.push(Hit { rect: Rect::new(body.x, y, body.width, shown), target: Target::EditCard(i) });
        y += h + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_api::{Event, StopReason, ToolCall, Usage};
    use crate::model::Role;
    use serde_json::json;

    fn entry(seq: u64, event: Event) -> LogEntry {
        LogEntry { seq, at_ms: 0, event }
    }

    /// A user message, an answer with one tool call, another user message.
    fn view() -> ChatEditView {
        let call = ToolCall { id: "c1".into(), name: "read".into(), input: json!({"path": "a"}) };
        let log = vec![
            entry(0, Event::UserMessage { text: "first".into() }),
            entry(1, Event::TurnStarted { turn: 1, provider: "p".into(), model: "m".into() }),
            entry(2, Event::TextDelta { text: "reading".into() }),
            entry(3, Event::ToolRequested { call: call.clone(), capabilities: vec![] }),
            entry(4, Event::TurnFinished { turn: 1, stop: StopReason::ToolUse, usage: Usage::default(), cost_micro_usd: None }),
            entry(5, Event::ToolResult { id: "c1".into(), output: "a's text".into(), is_error: false }),
            entry(6, Event::UserMessage { text: "second".into() }),
        ];
        let cards = vec![(CardKind::User, vec![0], vec![]), (CardKind::Answer, vec![1, 2, 4], vec![2]), (CardKind::Tool, vec![3, 5], vec![]), (CardKind::User, vec![6], vec![])];
        let back = crate::overlay::sample_settings();
        ChatEditView::new("s-1".into(), "the chat".into(), &log, cards, |_| String::new(), back)
    }

    fn workspace(e: ChatEditView) -> Workspace {
        let crate::model::Mode::Session(mut w) = crate::fixtures::main_session().mode else { unreachable!() };
        w.overlay = Some(Overlay::ChatEdit(e));
        w
    }

    fn editor(w: &Workspace) -> &ChatEditView {
        match &w.overlay {
            Some(Overlay::ChatEdit(e)) => e,
            other => panic!("not the editor: {other:?}"),
        }
    }

    #[test]
    fn the_cards_are_drawn_as_the_chat_draws_them() {
        let e = view();
        assert!(matches!(e.cards[0].items.as_slice(), [Item::Turn { role: Role::User, .. }]));
        assert!(matches!(e.cards[2].items.as_slice(), [Item::Tool(_)]), "a tool call is its own card: {:?}", e.cards[2].items);
        assert_eq!(e.last_seq, 6);
    }

    #[test]
    fn keys_move_mark_delete_and_bring_back() {
        let mut w = workspace(view());
        key(&mut w, Key::Down);
        assert_eq!(editor(&w).cursor, 1);
        key(&mut w, Key::Enter);
        key(&mut w, Key::Down);
        key(&mut w, Key::Down);
        key(&mut w, Key::Enter);
        assert_eq!(editor(&w).marked.iter().copied().collect::<Vec<_>>(), vec![1, 3]);
        key(&mut w, Key::Enter);
        assert_eq!(editor(&w).marked.len(), 1, "Enter again unmarks");
        // d with a mark: the answer goes with its tool call.
        key(&mut w, Key::Char('d'));
        assert_eq!(editor(&w).gone, vec![false, true, true, false]);
        assert_eq!(editor(&w).deleted_seqs(), vec![1, 2, 3, 4, 5]);
        // The cursor never rests on a deleted card.
        assert!(!editor(&w).gone[editor(&w).cursor]);
        key(&mut w, Key::Up);
        assert_eq!(editor(&w).cursor, 0, "Up passes the deleted cards");
        // d with no mark: the card under the cursor.
        key(&mut w, Key::Char('d'));
        assert_eq!(editor(&w).gone, vec![true, true, true, false]);
        key(&mut w, Key::Char('u'));
        key(&mut w, Key::Char('u'));
        assert_eq!(editor(&w).gone, vec![false; 4], "u brings back each deletion");
    }

    #[test]
    fn esc_asks_only_with_unsaved_deletions() {
        let mut w = workspace(view());
        key(&mut w, Key::Esc);
        assert!(matches!(w.overlay, Some(Overlay::Settings(_))), "nothing deleted: Esc leaves at once");
        let mut w = workspace(view());
        key(&mut w, Key::Char('d'));
        key(&mut w, Key::Esc);
        assert!(editor(&w).asking_leave);
        key(&mut w, Key::Char('n'));
        assert!(!editor(&w).asking_leave, "n stays");
        key(&mut w, Key::Esc);
        key(&mut w, Key::Char('y'));
        assert!(matches!(w.overlay, Some(Overlay::Settings(_))));
        assert!(w.outbox.iter().all(|o| !matches!(o, Outgoing::ChatEditSave { .. })), "leaving saves nothing");
    }

    #[test]
    fn s_sends_one_save_of_the_deleted_seqs() {
        let mut w = workspace(view());
        key(&mut w, Key::Char('s'));
        assert!(w.outbox.is_empty(), "nothing deleted, nothing to save");
        assert!(editor(&w).status.contains("Nothing is deleted"));
        key(&mut w, Key::Down);
        key(&mut w, Key::Down);
        key(&mut w, Key::Char('d'));
        key(&mut w, Key::Char('s'));
        let saves: Vec<&Outgoing> = w.outbox.iter().filter(|o| matches!(o, Outgoing::ChatEditSave { .. })).collect();
        assert_eq!(saves.len(), 1);
        assert!(matches!(saves[0], Outgoing::ChatEditSave { session, last_seq: 6, deleted, cards: 1, .. } if session == "s-1" && deleted == &vec![3, 5]), "{saves:?}");
        assert!(matches!(&w.overlay, Some(Overlay::Settings(s)) if s.chats.status.contains("Saving")));
    }

    #[test]
    fn it_draws_at_80x24_with_a_hit_per_card() {
        use ratatui::{Terminal, backend::TestBackend};
        let e = view();
        let theme = crate::theme::Theme::builtin("dark").unwrap();
        for (wd, ht) in [(80, 24), (120, 40)] {
            let mut terminal = Terminal::new(TestBackend::new(wd, ht)).unwrap();
            let mut out = Drawn::default();
            terminal.draw(|f| draw(f, f.area(), &e, &theme, &mut out)).unwrap();
            let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
            assert!(screen.contains("Edit · the chat"), "{screen}");
            assert!(screen.contains("first") && screen.contains("second"));
            assert_eq!(out.hits.iter().filter(|h| matches!(h.target, Target::EditCard(_))).count(), 4);
        }
    }
}
