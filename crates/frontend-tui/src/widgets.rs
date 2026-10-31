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

/// The small multitool mark drawn in text cells: fork, handle with two rivets, saw.
/// Ten cells wide. The full-size logo is an image (docs/design/logo); this is its cell form.
pub fn logo_mark(t: &Theme) -> Vec<Span<'static>> {
    let tool = Style::default().fg(t.text);
    let handle = Style::default().fg(t.brand);
    vec![
        Span::styled("≡━", tool),
        Span::styled("◖●━━●◗", handle),
        Span::styled("◣◣", tool),
    ]
}

/// Top line: mark, product name, a muted location; on the right path, adapter, branch, status.
pub fn header(f: &mut Frame, area: Rect, t: &Theme, h: &HeaderInfo) {
    fill(f, area, t.background);
    let mut left = vec![Span::raw(" ")];
    left.extend(logo_mark(t));
    left.push(Span::raw("  "));
    left.push(Span::styled(h.product.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
    left.push(Span::styled(format!(" / {}", h.location), Style::default().fg(t.text_muted)));
    f.render_widget(Paragraph::new(Line::from(left)), area);

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
        spans.push(Span::styled(format!("{k}  "), Style::default().fg(t.text_muted)));
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

