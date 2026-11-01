//! Building blocks shared by every screen: header, cards, composer, key-hint footer.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::model::{Composer, HeaderInfo, KeyHint, StatusLine};
use crate::theme::Theme;

/// Fills `area` with one background colour.
pub fn fill(f: &mut Frame, area: Rect, bg: ratatui::style::Color) {
    f.render_widget(Block::default().style(Style::default().bg(bg)), area);
}

/// Cells the logo occupies at the start of the header (a picture only; no text fallback).
pub const LOGO_COLS: u16 = 11;

/// Top line: mark, product name, a muted location; on the right path, branch, status.
/// Returns the cells of the mark, where the logo picture goes.
/// `info` puts path, branch and status on the right; off when the tabs take that place.
pub fn header(f: &mut Frame, area: Rect, t: &Theme, h: &HeaderInfo, logo: bool, info: bool) -> Option<Rect> {
    fill(f, area, t.background);
    // With `logo`, cells 1..=LOGO_COLS are left for the picture; without, the name leads.
    let lead = if logo { LOGO_COLS as usize + 3 } else { 1 };
    let mut left = vec![Span::raw(" ".repeat(lead))];
    left.push(Span::styled(h.product.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
    left.push(Span::styled(format!(" / {}", h.location), Style::default().fg(t.text_muted)));
    f.render_widget(Paragraph::new(Line::from(left)), area);

    if info {
        let parts: Vec<&str> = std::iter::once(h.cwd.as_str()).chain(h.branch.as_deref()).collect();
        let sep = Span::styled("  |  ", Style::default().fg(t.text_muted));
        let mut right: Vec<Span> = Vec::new();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                right.push(sep.clone());
            }
            right.push(Span::styled(part.to_string(), Style::default().fg(t.text)));
        }
        right.push(sep.clone());
        let (dot, label) = if h.online { (t.success, "online") } else { (t.text_muted, "offline") };
        right.push(Span::styled("● ", Style::default().fg(dot)));
        right.push(Span::styled(format!("{label} "), Style::default().fg(t.text)));
        f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), area);
    }
    logo.then(|| Rect { x: area.x + 1, y: area.y, width: LOGO_COLS.min(area.width.saturating_sub(1)), height: 1 })
}

/// A horizontal rule in the subtle border colour.
pub fn rule(f: &mut Frame, area: Rect, t: &Theme) {
    let line = "─".repeat(area.width as usize);
    f.render_widget(Paragraph::new(line).style(Style::default().fg(t.border_subtle).bg(t.background)), area);
}

/// A card: surface background, an accent stripe on the left, a label line (spans), then body
/// lines, then one padding row. The stripe is a half block so it reads as a bar, not a border.
pub fn card(f: &mut Frame, area: Rect, t: &Theme, label: Vec<Span<'static>>, body: Vec<Line<'static>>) {
    fill(f, area, t.surface);
    let stripe = Rect { width: 1, ..area };
    let bar = vec![Line::from("▌"); area.height as usize];
    f.render_widget(Paragraph::new(bar).style(Style::default().fg(t.accent_line).bg(t.surface)), stripe);

    let inner = Rect { x: area.x + 3, y: area.y, width: area.width.saturating_sub(5), height: area.height.saturating_sub(1) };
    let mut lines = vec![Line::from(label)];
    lines.extend(body);
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.surface)), inner);
}

/// `LABEL value · LABEL value · …` above the composer.
pub fn status_line(f: &mut Frame, area: Rect, t: &Theme, s: &StatusLine) {
    let mut spans = vec![Span::raw(" ")];
    for (i, (k, v)) in s.fields.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(t.text_muted)));
        }
        // Labels bold and underlined, as every mockup's status line has them.
        spans.push(Span::styled(k.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD | Modifier::UNDERLINED)));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(v.clone(), Style::default().fg(t.text)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.background)), area);
}

/// A button beside the composer: the same height and rounded border, its label on the row of
/// the composer's text and its shortcut on the row of the composer's hints, so both boxes read
/// as one bar.
pub fn button(f: &mut Frame, area: Rect, t: &Theme, label: &str, shortcut: &str) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border_subtle))
        .style(Style::default().bg(t.background));
    let lines = vec![
        Line::from(Span::styled(label.to_string(), Style::default().fg(t.text))),
        Line::from(Span::styled(shortcut.to_string(), Style::default().fg(t.text_muted))),
    ];
    f.render_widget(Paragraph::new(lines).centered().block(block), area);
}

/// The input box: accent border, placeholder in muted text, a block cursor.
pub fn composer(f: &mut Frame, area: Rect, t: &Theme, c: &Composer) {
    let border = if c.focused { t.accent_line } else { t.border_subtle };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border))
        .style(Style::default().bg(t.background));
    let mut spans = vec![Span::raw(" ")];
    if c.text.is_empty() {
        spans.push(Span::styled(c.placeholder.clone(), Style::default().fg(t.text_muted)));
    } else {
        spans.push(Span::styled(c.text.clone(), Style::default().fg(t.text)));
    }
    if c.focused {
        spans.push(Span::styled("█", Style::default().fg(t.accent_line)));
    }
    let lines = vec![Line::from(spans), hint_line(t, &c.hints)];
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// `key label · key label · …` in muted text, keys a step brighter.
pub fn hint_line(t: &Theme, items: &[KeyHint]) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, h) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(t.text_muted)));
        }
        spans.push(Span::styled(h.key.clone(), Style::default().fg(t.text_muted)));
        if !h.label.is_empty() {
            spans.push(Span::styled(format!(" {}", h.label), Style::default().fg(t.text_muted)));
        }
    }
    Line::from(spans)
}

/// A run's seconds as the activity line says them.
pub fn run_time(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 { format!("{s}s") } else { format!("{}m {:02}s", s / 60, s % 60) }
}

/// What the agent is doing, for how long, and how to stop it: at the right of the status line.
pub fn activity(f: &mut Frame, status: Rect, t: &Theme, a: &crate::model::Activity) {
    const SPINNER: [&str; 8] = ["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];
    let frame = (a.now_ms / crate::live::SPIN_MS) as usize % SPINNER.len();
    let time = run_time(a.now_ms.saturating_sub(a.started_ms));
    let spans = vec![
        Span::styled(format!(" {} ", SPINNER[frame]), Style::default().fg(t.accent)),
        Span::styled(a.state.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {time}"), Style::default().fg(t.accent)),
        Span::styled("  ·  Esc stops ", Style::default().fg(t.text_muted)),
    ];
    // In cells: a moon takes two.
    let width = (spans.iter().flat_map(|s| s.content.chars()).map(crate::markdown::cell_width).sum::<usize>() as u16).min(status.width);
    let area = Rect::new(status.x + status.width - width, status.y, width, 1);
    f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.background)), area);
}

/// How many queued messages show, the rest said in a line.
const QUEUE_ROWS: usize = 4;

pub fn queue_height(queued: &[String]) -> u16 {
    queued.len().min(QUEUE_ROWS + 1) as u16
}

/// Messages waiting for the agent, dim, above the composer, newest last: it sees them at its next
/// step. The rows are one line each, whatever a message's length.
pub fn queue(f: &mut Frame, area: Rect, t: &Theme, queued: &[String]) {
    if queued.is_empty() {
        return;
    }
    let muted = Style::default().fg(t.text_muted);
    let shown = if queued.len() > QUEUE_ROWS + 1 { QUEUE_ROWS } else { queued.len() };
    let skipped = queued.len() - shown;
    let mut lines = Vec::new();
    if skipped > 0 {
        lines.push(Line::from(Span::styled(format!("   … {skipped} more before these"), muted)));
    }
    for (i, text) in queued[skipped..].iter().enumerate() {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut spans = vec![Span::styled(" ⧗ ", Style::default().fg(t.accent)), Span::styled(flat, muted)];
        if skipped + i + 1 == queued.len() {
            spans.push(Span::styled("  ·  ↑ to change", muted.add_modifier(Modifier::DIM)));
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.background)), area);
}

/// The rows above the composer: an open question, once the run has ended, or the queue.
pub fn panel_height(v: &crate::model::SessionView, width: u16) -> u16 {
    match &v.question {
        Some((question, _)) if !v.generating => (question_lines(&question.text, width).len() + question.options.len() + 1) as u16,
        _ => queue_height(&v.queued),
    }
}

/// The longest a question grows, whatever a model writes: the card above holds all of it.
const QUESTION_ROWS: usize = 6;

fn question_lines(text: &str, width: u16) -> Vec<String> {
    let room = (width as usize).saturating_sub(4).max(10);
    let mut lines: Vec<String> = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > room {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines.truncate(QUESTION_ROWS);
    lines
}

/// What the agent asked, above the composer: its options to pick from, or the composer for a
/// free answer.
pub fn question(f: &mut Frame, area: Rect, t: &Theme, question: &harness_api::question::Question, cursor: usize) {
    let (accent, muted) = (Style::default().fg(t.accent), Style::default().fg(t.text_muted));
    let mut lines: Vec<Line> = question_lines(&question.text, area.width)
        .into_iter()
        .enumerate()
        .map(|(i, text)| Line::from(vec![Span::styled(if i == 0 { " ? " } else { "   " }, accent), Span::styled(text, Style::default().fg(t.text).add_modifier(Modifier::BOLD))]))
        .collect();
    for (i, option) in question.options.iter().enumerate() {
        let here = i == cursor;
        let mut spans = vec![
            Span::styled(if here { " ❯ " } else { "   " }, accent),
            Span::styled(format!("{}. {}", i + 1, option.label), if here { accent.add_modifier(Modifier::BOLD) } else { Style::default().fg(t.text) }),
        ];
        if let Some(description) = &option.description {
            spans.push(Span::styled(format!("  {description}"), muted));
        }
        lines.push(Line::from(spans));
    }
    let hint = if question.options.is_empty() { "   type your answer below" } else { "   ↑↓ and Enter, or a number  ·  or type your own answer" };
    lines.push(Line::from(Span::styled(hint, muted.add_modifier(Modifier::DIM))));
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.background)), area);
}
