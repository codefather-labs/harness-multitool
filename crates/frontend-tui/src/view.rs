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
    let want = (v.scroll as u16).min(5000);
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
    if let (Some(toast), None) = (&w.toast, &w.overlay) {
        draw_toast(f, tr, toast, t, out);
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
        // A message about what is going on in the overlay has to show over it.
        if let Some(toast) = &w.toast {
            draw_toast(f, tr, toast, t, out);
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
            TabMark::Unread(n) => format!(" ●{n}"),
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
fn draw_toast(f: &mut Frame, area: Rect, toast: &crate::model::Toast, t: &Theme, out: &mut Drawn) {
    use crate::images::{Icon, Tint};
    use crate::model::InboundKind;
    use ratatui::widgets::{Block, BorderType, Borders};
    let (name, color, icon, tint) = match toast.kind {
        InboundKind::Telegram | InboundKind::Voice { .. } => ("Telegram", t.info, Icon::Send, Tint::Info),
        InboundKind::Agent { .. } => ("Agent", t.agent, Icon::Bot, Tint::Agent),
        InboundKind::Callback { .. } => ("Callback", t.success, Icon::Callback, Tint::Success),
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
