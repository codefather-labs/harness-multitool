//! Draws an `AppState`. The only drawing entry point of the terminal frontend: the live CLI and
//! the snapshot tool both call `draw`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::images::{Graphic, Placement};
use crate::model::{AppState, Mode, Workspace};
use crate::theme::Theme;
use crate::widgets;

/// What a draw produced besides cells: where pictures go and which areas react to the mouse.
#[derive(Debug, Default)]
pub struct Drawn {
    pub placements: Vec<Placement>,
    pub hits: Vec<Hit>,
}

/// A clickable area and what it stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub rect: Rect,
    pub target: Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    SplashOk,
    Session(usize),
    Action(usize),
    Settings,
    /// A session tab of the header.
    Tab(usize),
    /// The `+` after the tabs.
    NewTab,
    /// The `×` of the toast.
    CloseToast,
    /// The transcript (wheel scrolling).
    Transcript,
    /// A button of the permission prompt: 0 allow once, 1 always allow, 2 deny.
    Choice(usize),
    /// A section of the settings window.
    Section(usize),
    /// The sign-in button of the Provider page.
    AuthButton,
    /// The model row: opens the dropdown.
    ModelRow,
    /// A row of the open dropdown.
    ModelChoice(usize),
    ContextRow,
    DelegateOwn,
    DelegateSkip,
    ContextChoice(usize),
    /// A row of the settings' Speech to text page.
    VoiceRow(usize),
    /// A choice in that page's open dropdown.
    VoiceChoice(usize),
    /// A line of the Profiles tab.
    ProfileRow(usize),
    /// A button of the Profiles tab: 0 Add, 1 Edit, 2 Delete, 3 Use; in Edit 10 sign Claude Code in, 11 out.
    ProfileButton(usize),
    /// The Chats page (CON-173): a conversation's row, its Dump button, Load.
    ChatRow(usize),
    ChatDump,
    ChatLoad,
    /// A provider row on the settings window's provider page.
    Provider(usize),
    /// A row of the settings' Callbacks tab.
    CallbackRow(usize),
    /// The close mark of an overlay.
    CloseOverlay,
    /// A clickable part of a map in the transcript: item index, the attached item inside a
    /// turn, the part.
    MapPart { item: usize, sub: Option<usize>, element: crate::model::MapElement },
}

impl Drawn {
    /// The target under a cell, the last drawn first.
    pub fn hit(&self, col: u16, row: u16) -> Option<Target> {
        self.hits
            .iter()
            .rev()
            .find(|h| col >= h.rect.x && col < h.rect.x + h.rect.width && row >= h.rect.y && row < h.rect.y + h.rect.height)
            .map(|h| h.target)
    }
}

/// Draws the state. Cells under a placement hold a text fallback, so a terminal without graphics
/// still shows something sensible.
pub fn draw(f: &mut Frame, s: &AppState, t: &Theme) -> Drawn {
    // Whatever else breaks in a frame, the session goes on behind it (CON-113).
    match crate::transcript::guarded(|| draw_all(f, s, t)) {
        Some(drawn) => drawn,
        None => {
            let area = f.area();
            widgets::fill(f, area, t.background);
            f.render_widget(ratatui::widgets::Paragraph::new("This frame could not be drawn; the error is in draw-errors.log in the harness's data folder. The session goes on."), area);
            Drawn::default()
        }
    }
}

fn draw_all(f: &mut Frame, s: &AppState, t: &Theme) -> Drawn {
    let area = f.area();
    widgets::fill(f, area, t.background);
    let mut out = Drawn::default();
    match &s.mode {
        Mode::Splash(v) => crate::home::splash(f, area, v, t, s.pictures, &mut out),
        Mode::Startup(v) => crate::home::startup(f, area, v, t, s.pictures, &mut out),
        Mode::Session(w) => workspace(f, area, s, w, t, &mut out),
    }
    // The logo's eyes are one state for the whole screen (the launch blink).
    for p in &mut out.placements {
        if let Graphic::Logo(eyes) = &mut p.graphic {
            *eyes = s.eyes;
        }
    }
    // Every picture's cells get its text fallback; a graphics terminal draws the picture over it.
    for p in &out.placements {
        crate::images::fallback(f.buffer_mut(), p, t);
    }
    out
}

fn inset(r: Rect) -> Rect {
    Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r }
}

/// Session chrome (journal: one session chrome): header with the tabs, transcript, rule,
/// status line, composer with key hints inside, the Settings button to its right; a toast over
/// the top right of the transcript.
fn workspace(f: &mut Frame, area: Rect, s: &AppState, w: &Workspace, t: &Theme, out: &mut Drawn) {
    let [head, rule, body] = Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    let v = w.view();
    let mut header = s.header.clone();
    if w.tabs.len() > 1 {
        header.location = format!("session/{}", w.tabs[w.active].title);
    }
    let logo = widgets::header(f, head, t, &header, s.pictures, w.tabs.len() <= 1);
    if w.tabs.len() > 1 {
        tabs(f, head, w, t, s.pictures, out);
    }
    widgets::rule(f, rule, t);
    if let Some(rect) = logo {
        out.placements.push(Placement { rect, graphic: Graphic::Logo(s.eyes) });
    }
    let [transcript, rule, status, queue, bottom] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(widgets::panel_height(v, area.width)),
        Constraint::Length(widgets::composer_height(&v.composer, inset(area).width.saturating_sub(15))),
    ])
    .areas(body);
    let tr = inset(transcript);
    let want = (v.scroll as u16).min(60_000);
    let (drawn, whole) = crate::transcript::visible(&v.transcript, tr.width, t, &w.journal, v.agent_name.as_deref().unwrap_or(crate::transcript::ASSISTANT), tr.height.saturating_add(want));
    let total = crate::transcript::total_height(&drawn);
    // Scrolling stops at the first item; while older items exist beyond the stack, the stack
    // already covers the rows asked for.
    let scroll = if whole { want.min(total.saturating_sub(tr.height)) } else { want };
    let (pictures, map_hits) = crate::transcript::blit(f.buffer_mut(), tr, drawn, scroll);
    out.placements.extend(pictures);
    out.hits.push(Hit { rect: tr, target: Target::Transcript });
    // Map parts after the transcript itself, so they win where they lie.
    for (rect, item, sub, element) in map_hits {
        out.hits.push(Hit { rect, target: Target::MapPart { item, sub, element } });
    }
    if scroll > 0 {
        let more = Rect { y: tr.y + tr.height.saturating_sub(1), height: 1, ..tr };
        f.render_widget(
            Paragraph::new(Span::styled(format!(" ↓ {scroll} more rows · End to jump back "), Style::default().fg(t.accent_text).bg(t.accent)))
                .right_aligned(),
            more,
        );
    }
    // The reconnecting toast stands at the top; any other toast goes under it (CON-131).
    let mut below = match &w.reconnecting {
        Some(r) => draw_reconnecting(f, tr, r, t),
        None => 0,
    };
    // Under it, a toast for each conversation being moved (CON-144).
    below += draw_moving(f, Rect { y: tr.y + below, height: tr.height.saturating_sub(below), ..tr }, &w.moving, t);
    // Then the Chats tab's dumps and loads (CON-172).
    if w.overlay.is_none() {
        below += draw_progress(f, Rect { y: tr.y + below, height: tr.height.saturating_sub(below), ..tr }, &w.archiving, t, "under way");
    }
    let tr_toast = Rect { y: tr.y + below, height: tr.height.saturating_sub(below), ..tr };
    if let (Some(toast), None) = (&w.toast, &w.overlay) {
        draw_toast(f, tr_toast, toast, t, out);
    }
    widgets::rule(f, rule, t);
    widgets::status_line(f, status, t, &v.status);
    if let Some(a) = &v.activity {
        widgets::activity(f, status, t, a);
    }
    let running = widgets::background_height(&v.background).min(queue.height);
    widgets::background(f, Rect { height: running, ..queue }, t, &v.background, v.background_now_ms);
    let queue = Rect { y: queue.y + running, height: queue.height - running, ..queue };
    match &v.question {
        Some((question, cursor)) if !v.generating => widgets::question(f, queue, t, question, *cursor),
        _ => widgets::queue(f, queue, t, &v.queued),
    }
    let [input, _, button] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(14)]).areas(inset(bottom));
    widgets::composer(f, input, t, &v.composer);
    let button = Rect { height: button.height.min(4), ..button };
    widgets::button(f, button, t, "⚙ Settings", "Ctrl+,");
    out.hits.push(Hit { rect: button, target: Target::Settings });
    // An overlay takes the clicks: only its own targets stay live.
    if let Some(o) = &w.overlay {
        out.hits.clear();
        crate::overlay::draw(f, area, o, t, out);
        // A message about what is going on in the overlay has to show over it, and so does a
        // dump's or a load's progress (CON-172).
        let over = draw_progress(f, tr, &w.archiving, t, "under way");
        if let Some(toast) = &w.toast {
            draw_toast(f, Rect { y: tr.y + over, height: tr.height.saturating_sub(over), ..tr }, toast, t, out);
        }
    }
}

/// The tab strip at the right of the header (session-tabs-*, journal#137): one pill per session
/// with its icon and mark, the active one in the accent, `+` at the end.
fn tabs(f: &mut Frame, head: Rect, w: &Workspace, t: &Theme, pictures: bool, out: &mut Drawn) {
    use crate::images::{Icon, Tint};
    use crate::model::{TabIcon, TabMark};
    let bg = t.background;
    let mut x = head.x + head.width;
    // Each tab: its pill's spans, its icon picture (offset, icon, tint), its click target.
    type TabItem = (Vec<Span<'static>>, Option<(u16, Icon, Tint)>, Target);
    let mut groups: Vec<bool> = Vec::new();
    let mut items: Vec<TabItem> = Vec::new();
    let plus_fill = t.surface_raised;
    items.push((crate::transcript::pill_spans(" + ", t.text, plus_fill, bg), None, Target::NewTab));
    groups.push(false);
    for (i, tab) in w.tabs.iter().enumerate().rev() {
        let active = i == w.active;
        // An orchestra's tabs read as one group (CON-100).
        let grouped = tab.view.orchestra.is_some();
        let fill = if active {
            crate::transcript::tint(t.accent, bg, 0.2)
        } else if grouped {
            crate::transcript::tint(t.info, bg, 0.25)
        } else {
            t.surface_raised
        };
        let fg = if active { t.accent } else { t.text };
        let (icon, glyph) = match tab.icon {
            TabIcon::Prompt => (Icon::Terminal, ">_"),
            TabIcon::Agent => (Icon::Swap, "⇄ "),
            TabIcon::Doc => (Icon::File, "▤ "),
            TabIcon::Lock => (Icon::Lock, "■ "),
        };
        let mark = match tab.mark {
            TabMark::None => String::new(),
            TabMark::Busy => " ◌".into(),
            TabMark::Unread(n) => format!(" ● {n}"),
            TabMark::Attention => " !".into(),
        };
        let lead = if pictures { "    ".to_string() } else { format!(" {glyph} ") };
        let place = match tab.view.orchestra.as_deref() {
            Some("master") => " (M)",
            Some("slave") => " (S)",
            _ => "",
        };
        let text = format!("{lead}{}{place}{mark} ", tab.title);
        let mut spans = crate::transcript::pill_spans(&text, fg, fill, bg);
        // In a group the ends are solid, so the tabs read as one bar.
        if grouped {
            let last = spans.len() - 1;
            spans[0].style = spans[0].style.bg(fill);
            spans[last].style = spans[last].style.bg(fill);
        }
        if let Some(m) = spans.get_mut(1) {
            m.style = m.style.add_modifier(Modifier::BOLD);
        }
        let mark_color = match tab.mark {
            TabMark::Busy => Some(t.info),
            TabMark::Unread(_) => Some(t.info),
            TabMark::Attention => Some(t.warning),
            TabMark::None => None,
        };
        if let (Some(c), false) = (mark_color, mark.is_empty()) {
            // Split the mark off so it takes its own colour.
            let body = spans[1].content.to_string();
            let cut = body.len() - mark.len() - 1;
            let style = spans[1].style;
            spans[1] = Span::styled(body[..cut].to_string(), style);
            spans.insert(2, Span::styled(mark.clone(), style.fg(c)));
            spans.insert(3, Span::styled(" ", style));
        }
        groups.push(grouped);
        items.push((spans, pictures.then_some((2, icon, if active { Tint::Accent } else { Tint::Text })), Target::Tab(i)));
    }
    let mut group_on_right = false;
    for (index, (spans, icon, target)) in items.into_iter().enumerate() {
        let width: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        if x < head.x + 40 + width {
            break;
        }
        let in_group = groups.get(index).copied().unwrap_or(false);
        // Inside a group the tabs touch.
        x -= width + u16::from(!(in_group && group_on_right));
        group_on_right = in_group;
        let r = Rect { x, width, ..head };
        f.render_widget(Paragraph::new(Line::from(spans)), r);
        if let Some((off, icon, tint)) = icon {
            out.placements.push(Placement { rect: Rect { x: x + off, width: 2, ..head }, graphic: Graphic::Icon(icon, tint) });
        }
        out.hits.push(Hit { rect: r, target });
    }
}

/// An inbound message that arrived while the agent was busy, over the top right (the toast of
/// inbound-messages-*): icon, `Channel · sender  now`, the text, `×` to close.
/// Where the lit part of the reconnecting bar stands at `now_ms`: it sweeps from end to end and
/// back, a step every 80 ms.
pub fn pulse_at(width: u16, lit: u16, now_ms: u64) -> u16 {
    let span = width.saturating_sub(lit).max(1) as u64;
    let step = (now_ms / 80) % (2 * span);
    (if step < span { step } else { 2 * span - step }) as u16
}

/// The reconnecting toast (CON-131): 'Reconnecting to the daemon… attempt N', the reason, and a
/// pulsing bar. Answers the rows it took.
fn draw_reconnecting(f: &mut Frame, area: Rect, r: &crate::model::Reconnecting, t: &Theme) -> u16 {
    use ratatui::widgets::{Block, BorderType, Borders};
    let w = 46.min(area.width);
    let rect = Rect { x: area.x + area.width.saturating_sub(w), y: area.y, width: w, height: 5.min(area.height) };
    f.render_widget(ratatui::widgets::Clear, rect);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.warning)).style(Style::default().bg(t.surface));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let attempt = if r.attempt == 0 { String::new() } else { format!(" · attempt {}", r.attempt) };
    let bar_w = inner.width.saturating_sub(2);
    let lit = (bar_w / 5).max(2);
    let at = pulse_at(bar_w, lit, r.now_ms);
    let bar: String = (0..bar_w).map(|i| if i >= at && i < at + lit { '█' } else { '░' }).collect();
    let reason = if r.reason.is_empty() { "no connection".to_string() } else { r.reason.clone() };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Reconnecting to the daemon…", Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(attempt, Style::default().fg(t.text_muted)),
            ]),
            Line::from(Span::styled(format!(" {reason}"), Style::default().fg(t.text_muted))),
            Line::from(Span::styled(format!(" {bar}"), Style::default().fg(t.warning))),
        ]),
        inner,
    );
    rect.height
}

/// How many moving conversations get a toast of their own; the rest are counted.
pub const MOVING_SHOWN: usize = 3;

/// The words and the bar of a moving conversation's toast, for a bar `width` cells wide: what is
/// done; the count, its share and the tab's name; the bar. A line too long ends in an ellipsis.
pub fn moving_lines(m: &crate::model::Moving, width: u16) -> (String, String, String) {
    let fit = |text: String| {
        let room = usize::from(width) + 1;
        if text.chars().count() <= room { text } else { format!("{}…", text.chars().take(room.saturating_sub(1)).collect::<String>()) }
    };
    let head = fit(format!(" {}", m.what));
    let count = |n: u64| if m.bytes { format!("{:.0} MB", n as f64 / 1e6) } else { n.to_string() };
    let unit = if m.bytes { "" } else { " messages" };
    let numbers = if m.total == 0 {
        if m.done == 0 { "starting…".to_string() } else { format!("{}{unit}", count(m.done)) }
    } else {
        let share = m.done.min(m.total) * 100 / m.total.max(1);
        format!("{} / {}{unit} · {share}%", count(m.done), count(m.total))
    };
    let detail = fit(if m.name.is_empty() { format!(" {numbers}") } else { format!(" {numbers} · {}", m.name) });
    let bar: String = if m.total == 0 {
        let lit = (width / 5).max(2);
        let at = pulse_at(width, lit, m.now_ms);
        (0..width).map(|i| if i >= at && i < at + lit { '█' } else { '░' }).collect()
    } else {
        let filled = (u64::from(width) * m.done.min(m.total) / m.total.max(1)) as u16;
        (0..width).map(|i| if i < filled { '█' } else { '░' }).collect()
    };
    (head, detail, format!(" {bar}"))
}

/// The toasts of the conversations being moved (CON-144), one under another, the rest counted.
/// Answers the rows they took.
fn draw_moving(f: &mut Frame, area: Rect, moving: &[crate::model::Moving], t: &Theme) -> u16 {
    draw_progress(f, area, moving, t, "being moved")
}

/// Progress toasts, one under another, the rest counted as `+N more <rest>`. Answers the rows they took.
fn draw_progress(f: &mut Frame, area: Rect, moving: &[crate::model::Moving], t: &Theme, rest: &str) -> u16 {
    use ratatui::widgets::{Block, BorderType, Borders};
    let mut y = 0;
    for m in moving.iter().take(MOVING_SHOWN) {
        if y + 5 > area.height {
            break;
        }
        let w = 52.min(area.width);
        let rect = Rect { x: area.x + area.width.saturating_sub(w), y: area.y + y, width: w, height: 5 };
        f.render_widget(ratatui::widgets::Clear, rect);
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent)).style(Style::default().bg(t.surface));
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let (head, detail, bar) = moving_lines(m, inner.width.saturating_sub(2));
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(head, Style::default().fg(t.text).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(detail, Style::default().fg(t.text_muted))),
                Line::from(Span::styled(bar, Style::default().fg(t.accent))),
            ]),
            inner,
        );
        y += 5;
    }
    if moving.len() > MOVING_SHOWN && y < area.height {
        let more = format!(" +{} more {rest} ", moving.len() - MOVING_SHOWN);
        let w = (more.chars().count() as u16).min(area.width);
        f.render_widget(Paragraph::new(Span::styled(more, Style::default().fg(t.text_muted).bg(t.surface))), Rect { x: area.x + area.width.saturating_sub(w), y: area.y + y, width: w, height: 1 });
        y += 1;
    }
    y
}

fn draw_toast(f: &mut Frame, area: Rect, toast: &crate::model::Toast, t: &Theme, out: &mut Drawn) {
    use crate::images::{Icon, Tint};
    use crate::model::InboundKind;
    use ratatui::widgets::{Block, BorderType, Borders};
    let (name, color, icon, tint) = match toast.kind {
        InboundKind::Telegram | InboundKind::Voice { .. } => ("Telegram", t.info, Icon::Send, Tint::Info),
        InboundKind::Agent { .. } => ("Agent", t.agent, Icon::Bot, Tint::Agent),
        InboundKind::Callback { .. } => ("Callback", t.success, Icon::Callback, Tint::Success),
        InboundKind::Game { .. } => ("Game", t.warning, Icon::Callback, Tint::Accent),
        InboundKind::Reminder { .. } => ("Reminder", t.agent, Icon::Calendar, Tint::Agent),
        InboundKind::System => ("Harness", t.warning, Icon::Bolt, Tint::Agent),
    };
    let text_w = toast.text.chars().count().max(name.len() + toast.from.len() + 12) as u16;
    let w = (text_w + 12).min(area.width / 2).max(30.min(area.width));
    // The text wraps rather than being cut: the frame, the header line, then as many lines as it
    // takes, with one to spare since words do not break where the count says.
    let text_room = w.saturating_sub(11).max(1) as usize;
    let chars = toast.text.chars().count();
    let lines = if chars <= text_room { 1 } else { chars.div_ceil(text_room) + 1 } as u16;
    let r = Rect { x: area.x + area.width.saturating_sub(w), y: area.y, width: w, height: (3 + lines).min(area.height) };
    f.render_widget(ratatui::widgets::Clear, r);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(t.surface));
    let inner = block.inner(r);
    f.render_widget(block, r);
    out.placements.push(Placement { rect: Rect { x: inner.x + 1, y: inner.y, width: 3, height: 2 }, graphic: Graphic::Icon(icon, tint) });
    let text = Rect { x: inner.x + 6, width: inner.width.saturating_sub(9), ..inner };
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(name, Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!(" · {}  now", toast.from), Style::default().fg(t.text_muted)),
            ]),
            Line::from(Span::styled(toast.text.clone(), Style::default().fg(t.text))),
        ])
        .wrap(ratatui::widgets::Wrap { trim: true }),
        text,
    );
    let close = Rect { x: inner.x + inner.width.saturating_sub(2), y: inner.y, width: 1, height: 1 };
    f.render_widget(Paragraph::new(Span::styled("×", Style::default().fg(t.text_muted))), close);
    out.hits.push(Hit { rect: close, target: Target::CloseToast });
}

#[cfg(test)]
mod reconnecting_tests {
    use super::*;

    /// CON-172: a dump's progress toast shows over the Settings, and the Chats page's status wraps
    /// so a refusal's reason is read whole (journal#1278).
    #[test]
    fn a_dumps_progress_shows_over_the_settings() {
        let mut state = crate::fixtures::settings_overlay();
        let crate::model::Mode::Session(w) = &mut state.mode else { panic!() };
        let crate::model::Overlay::Settings(s) = w.overlay.as_mut().unwrap() else { panic!() };
        s.section = s.sections.iter().position(|n| n == crate::overlay::CHATS_SECTION).unwrap();
        s.chats.status = "Not dumped: /home/codefather-server/Documents/projects/fbscout/orchestrator-3-5b5fb5d6.zip is there already, and is never written over: END-OF-REASON".to_string();
        w.chats = Some(s.chats.clone());
        w.archiving.push(crate::model::Moving { tab: 1, name: "orchestrator-3".into(), what: "Dumping orchestrator-3".into(), done: 500_000_000, total: 1_000_000_000, bytes: true, ..Default::default() });
        let theme = crate::theme::Theme::builtin("dark").unwrap();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(140, 40)).unwrap();
        terminal.draw(|f| { draw(f, &state, &theme); }).unwrap();
        let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("Dumping orchestrator-3"), "the toast over the Settings");
        assert!(screen.contains("500 MB / 1000 MB · 50%"));
        let rows: Vec<String> = screen.chars().collect::<Vec<_>>().chunks(140).map(|r| r.iter().collect()).collect();
        assert!(screen.contains("END-OF-REASON"), "the status read to its end:\n{}", rows.join("\n"));
    }

    /// CON-144: a moving conversation's toast says what is done, the count and its share, with a
    /// bar filled to it; with no total yet its bar pulses; past 3 the rest are counted.
    #[test]
    fn a_moving_toast_counts_and_fills_its_bar() {
        let m = crate::model::Moving { tab: 1, name: "fix login".into(), what: "Writing Codex's copy".into(), to: "Codex".into(), done: 3200, total: 10000, bytes: false, now_ms: 0 };
        let (head, detail, bar) = moving_lines(&m, 48);
        assert_eq!(head, " Writing Codex's copy");
        assert_eq!(detail, " 3200 / 10000 messages · 32% · fix login");
        let bar20 = moving_lines(&m, 20).2;
        assert_eq!(bar20, format!(" {}{}", "█".repeat(6), "░".repeat(14)));
        assert_eq!(bar.chars().count(), 49);
        let read = crate::model::Moving { done: 410_000_000, total: 1_196_000_000, bytes: true, ..m.clone() };
        assert_eq!(moving_lines(&read, 48).1, " 410 MB / 1196 MB · 34% · fix login");
        let long = crate::model::Moving { name: "a very long name of a conversation that goes on".into(), ..m.clone() };
        let cut = moving_lines(&long, 30).1;
        assert!(cut.ends_with('…') && cut.chars().count() == 31, "{cut:?}");
        let full = crate::model::Moving { done: 10000, ..m.clone() };
        assert_eq!(moving_lines(&full, 10).2, format!(" {}", "█".repeat(10)));
        let unknown = crate::model::Moving { total: 0, done: 0, ..m.clone() };
        let (_, starting, a) = moving_lines(&unknown, 40);
        assert_eq!(starting, " starting… · fix login");
        let b = moving_lines(&crate::model::Moving { now_ms: 800, ..unknown.clone() }, 40).2;
        assert_ne!(a, b, "with no total the bar moves");
        let theme = crate::theme::Theme::builtin("dark").unwrap();
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 30)).unwrap();
        let five: Vec<_> = (0..5).map(|i| crate::model::Moving { tab: i, ..m.clone() }).collect();
        let mut rows = 0;
        terminal.draw(|f| rows = draw_moving(f, f.area(), &five, &theme)).unwrap();
        assert_eq!(rows, 16, "three toasts and the count");
        let screen: String = terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(screen.contains("+2 more being moved"), "{screen}");
    }

    /// CON-131: the reconnecting toast says the attempt and the reason, and its bar moves.
    #[test]
    fn the_reconnecting_toast_says_the_attempt_and_its_bar_pulses() {
        let theme = crate::theme::Theme::builtin("dark").unwrap();
        let draw = |now_ms: u64| {
            let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
            let r = crate::model::Reconnecting { attempt: 3, reason: "no daemon answers".into(), now_ms };
            terminal.draw(|f| { draw_reconnecting(f, f.area(), &r, &theme); }).unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height).map(|y| (0..buffer.area.width).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>()).collect::<Vec<_>>()
        };
        let first = draw(0);
        let text = first.join("\n");
        assert!(text.contains("Reconnecting to the daemon") && text.contains("attempt 3") && text.contains("no daemon answers"), "{text}");
        let bar = |rows: &[String]| rows.iter().find(|l| l.contains('█')).cloned().expect("a bar");
        let later = draw(800);
        assert_ne!(bar(&first).find('█'), bar(&later).find('█'), "the lit part moved");
        assert_eq!(pulse_at(20, 4, 0), 0);
        assert!((0..400).all(|n| pulse_at(20, 4, n * 80) <= 16), "it stays inside the bar");
    }
}
