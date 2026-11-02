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
    let color = if c.text.is_empty() { t.text_muted } else { t.text };
    c.drawn_width.set(area.width);
    let (shown, cursor) = composer_rows(c, area.width);
    let mut lines: Vec<Line> = shown
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = vec![Span::raw(" ")];
            let style = Style::default().fg(color);
            match cursor.filter(|(at_row, _)| *at_row == i && c.focused) {
                Some((_, column)) => {
                    let chars: Vec<char> = row.chars().collect();
                    let (before, rest) = chars.split_at(column.min(chars.len()));
                    spans.push(Span::styled(before.iter().collect::<String>(), style));
                    match rest.split_first() {
                        // On a character it shows through, so the text stays readable.
                        Some((under, after)) => {
                            spans.push(Span::styled(under.to_string(), Style::default().fg(t.background).bg(t.accent_line)));
                            spans.push(Span::styled(after.iter().collect::<String>(), style));
                        }
                        None => spans.push(Span::styled("█", Style::default().fg(t.accent_line))),
                    }
                }
                None => spans.push(Span::styled(row, style)),
            }
            Line::from(spans)
        })
        .collect();
    lines.push(hint_line(t, &c.hints));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// As many rows of text as the composer grows to; beyond them the cursor's stay in view.
const COMPOSER_ROWS: usize = 8;

/// What fits a row in a frame this wide: the frame, the margin on the left.
fn text_width(frame_width: u16) -> usize {
    frame_width.saturating_sub(3).max(1) as usize
}

/// The rows of a text in a frame this wide, as (first character, characters): wrapped at spaces
/// where it can, every character kept, line breaks standing for themselves between rows.
pub fn text_rows(text: &str, frame_width: u16) -> Vec<(usize, usize)> {
    let width = if frame_width == 0 { usize::MAX } else { text_width(frame_width) };
    let mut rows = Vec::new();
    let mut start = 0;
    for line in text.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let (mut from, mut used, mut space) = (0, 0, None);
        for (i, ch) in chars.iter().enumerate() {
            let cell = crate::markdown::cell_width(*ch);
            if used + cell > width && i > from {
                // A space on the row keeps the word whole.
                let cut = space.filter(|s: &usize| *s + 1 > from && *s + 1 < i + 1).map_or(i, |s| s + 1);
                rows.push((start + from, cut - from));
                from = cut;
                used = chars[from..i].iter().copied().map(crate::markdown::cell_width).sum();
                space = None;
            }
            if *ch == ' ' {
                space = Some(i);
            }
            used += cell;
        }
        rows.push((start + from, chars.len() - from));
        start += chars.len() + 1;
    }
    rows
}

/// The row a character index stands on. At a wrap it counts as the start of the next.
pub fn row_of(rows: &[(usize, usize)], at: usize) -> usize {
    rows.iter().rposition(|(start, _)| *start <= at).unwrap_or(0)
}

/// The rows the composer shows in a frame this wide, and the cursor among them as (row, column).
fn composer_rows(c: &Composer, frame_width: u16) -> (Vec<String>, Option<(usize, usize)>) {
    let width = text_width(frame_width);
    if c.text.is_empty() {
        let rows = text_rows(&c.placeholder, frame_width);
        let chars: Vec<char> = c.placeholder.chars().collect();
        let shown: Vec<String> = rows.iter().take(COMPOSER_ROWS).map(|(s, n)| chars[*s..*s + *n].iter().collect()).collect();
        // Its cursor comes after the placeholder, which is not there to edit.
        let last = shown.len().saturating_sub(1);
        return (shown.clone(), Some((last, shown.last().map_or(0, |r| r.chars().count()))));
    }
    let chars: Vec<char> = c.text.chars().collect();
    let mut rows = text_rows(&c.text, frame_width);
    let at = c.cursor();
    let cells = |row: &(usize, usize)| chars[row.0..row.0 + row.1].iter().copied().map(crate::markdown::cell_width).sum::<usize>();
    // A full row leaves room for the cursor at the end.
    if c.focused && c.back == 0 && rows.last().is_some_and(|row| cells(row) >= width) {
        rows.push((chars.len(), 0));
    }
    let row = row_of(&rows, at);
    let first = row.saturating_sub(COMPOSER_ROWS - 1).max(rows.len().saturating_sub(COMPOSER_ROWS).min(row));
    let shown: Vec<String> = rows.iter().skip(first).take(COMPOSER_ROWS).map(|(s, n)| chars[*s..*s + *n].iter().collect()).collect();
    (shown, Some((row - first, at - rows[row].0)))
}

/// How tall the composer is in a frame this wide: its text, its hints, the frame.
pub fn composer_height(c: &Composer, frame_width: u16) -> u16 {
    composer_rows(c, frame_width).0.len() as u16 + 3
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

/// The phase of the moon the harness's spinner is at.
pub fn moon(now_ms: u64) -> &'static str {
    const SPINNER: [&str; 8] = ["🌑", "🌒", "🌓", "🌔", "🌕", "🌖", "🌗", "🌘"];
    SPINNER[(now_ms / crate::live::SPIN_MS) as usize % SPINNER.len()]
}

/// What the agent is doing, for how long, and how to stop it: at the right of the status line.
pub fn activity(f: &mut Frame, status: Rect, t: &Theme, a: &crate::model::Activity) {
    let time = run_time(a.now_ms.saturating_sub(a.started_ms));
    let spans = vec![
        Span::styled(format!(" {} ", moon(a.now_ms)), Style::default().fg(t.accent)),
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

/// The rows above the composer: what runs in the background, then an open question, once the run
/// has ended, or the queue.
pub fn panel_height(v: &crate::model::SessionView, width: u16) -> u16 {
    background_height(&v.background)
        + match &v.question {
            Some((question, _)) if !v.generating => (question_lines(&question.text, width).len() + question.options.len() + 1) as u16,
            _ => queue_height(&v.queued),
        }
}

/// How many background tasks show, the rest said in a line.
const BACKGROUND_ROWS: usize = 3;

pub fn background_height(tasks: &[crate::model::BackgroundView]) -> u16 {
    tasks.len().min(BACKGROUND_ROWS + 1) as u16
}

/// What runs in the background: a moon, what it is, for how long.
pub fn background(f: &mut Frame, area: Rect, t: &Theme, tasks: &[crate::model::BackgroundView], now_ms: u64) {
    if tasks.is_empty() {
        return;
    }
    let muted = Style::default().fg(t.text_muted);
    let shown = if tasks.len() > BACKGROUND_ROWS + 1 { BACKGROUND_ROWS } else { tasks.len() };
    let mut lines: Vec<Line> = tasks[..shown]
        .iter()
        .map(|task| {
            let flat = task.description.split_whitespace().collect::<Vec<_>>().join(" ");
            Line::from(vec![
                Span::styled(format!(" {} ", moon(now_ms)), Style::default().fg(t.accent)),
                Span::styled("in background  ", muted),
                Span::styled(flat, Style::default().fg(t.text)),
                Span::styled(format!("  {}", run_time(now_ms.saturating_sub(task.started_ms))), Style::default().fg(t.accent)),
            ])
        })
        .collect();
    if tasks.len() > shown {
        lines.push(Line::from(Span::styled(format!("   … {} more", tasks.len() - shown), muted)));
    }
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.background)), area);
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

#[cfg(test)]
mod composer_tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn composer(text: &str) -> Composer {
        Composer { text: text.to_string(), placeholder: "Ask".to_string(), focused: true, ..Default::default() }
    }

    #[test]
    fn long_text_wraps_and_keeps_every_character() {
        // A frame of width 8 has 5 cells for text.
        assert_eq!(text_rows("aa bb cc", 8), vec![(0, 3), (3, 5)]);
        assert_eq!(text_rows("abcdefgh", 6), vec![(0, 3), (3, 3), (6, 2)]);
        assert_eq!(text_rows("a\n\nb", 6), vec![(0, 1), (2, 0), (3, 1)]);
        let text = "слово и ещё одно длинное слово, end ";
        let total: usize = text_rows(text, 10).iter().map(|r| r.1).sum();
        assert_eq!(total, text.chars().count());
    }

    #[test]
    fn the_composer_grows_with_its_text_up_to_a_limit() {
        assert_eq!(composer_height(&composer(""), 40), 4, "as it always was, empty");
        assert_eq!(composer_height(&composer("one line"), 40), 4);
        assert_eq!(composer_height(&composer("two\nlines"), 40), 5);
        assert_eq!(composer_height(&composer(&"word ".repeat(30)), 40), 8);
        assert_eq!(composer_height(&composer(&"word ".repeat(500)), 40), COMPOSER_ROWS as u16 + 3);
        assert_eq!(composer_height(&composer(&"x".repeat(37)), 40), 5, "a full row leaves room for the cursor");
    }

    #[test]
    fn nothing_leaves_the_frame_and_the_end_stays_in_view() {
        let t = crate::theme::Theme::builtin("dark").unwrap();
        let mut text = "word ".repeat(200);
        text.push_str("THEEND");
        let c = composer(&text);
        let (w, h) = (40, composer_height(&c, 40));
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| composer_draw(f, &t, &c)).unwrap();
        let buf = term.backend().buffer().clone();
        let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        let all: Vec<String> = (0..h).map(row).collect();
        for line in &all[1..all.len() - 1] {
            assert!(line.ends_with('│') && line.starts_with('│'), "{line}");
        }
        assert!(all.iter().any(|l| l.contains("THEEND█")), "{all:#?}");
    }

    fn composer_draw(f: &mut Frame, t: &crate::theme::Theme, c: &Composer) {
        let area = f.area();
        super::composer(f, area, t, c);
    }

    #[test]
    fn the_background_panel_lists_tasks_and_their_time() {
        use crate::model::BackgroundView;
        let t = crate::theme::Theme::builtin("dark").unwrap();
        let tasks: Vec<BackgroundView> = (0..5).map(|i| BackgroundView { id: i.to_string(), description: format!("job {i}"), started_ms: 1000 }).collect();
        assert_eq!(background_height(&[]), 0);
        assert_eq!(background_height(&tasks[..3]), 3);
        assert_eq!(background_height(&tasks[..4]), 4);
        assert_eq!(background_height(&tasks), 4);
        let mut term = Terminal::new(TestBackend::new(60, 4)).unwrap();
        term.draw(|f| { let area = f.area(); background(f, area, &t, &tasks, 8000) }).unwrap();
        let buf = term.backend().buffer().clone();
        let row = |y: u16| (0..60).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert!(row(0).contains("in background") && row(0).contains("job 0") && row(0).contains("7s"), "{}", row(0));
        assert!(row(3).contains("2 more"), "{}", row(3));
    }

    #[test]
    fn the_cursor_moves_every_way_and_typing_goes_where_it_is() {
        let mut c = composer("héllo");
        c.left();
        c.left();
        c.insert("X");
        assert_eq!(c.text, "hélXlo");
        c.backspace();
        assert_eq!(c.text, "héllo");
        c.delete();
        assert_eq!(c.text, "hélo");
        assert_eq!(c.cursor(), 3);
        c.right();
        c.right();
        c.right();
        assert_eq!(c.back, 0, "stops at the end");
        for _ in 0..9 {
            c.left();
        }
        assert_eq!(c.cursor(), 0, "stops at the start");
        c.backspace();
        assert_eq!(c.text, "hélo");
        c.end();
        assert_eq!(c.back, 0);
        c.home();
        assert_eq!(c.cursor(), 0);
        c.text = "abcd\nef\nghij".into();
        c.back = 0;
        assert_eq!(c.cursor(), 12);
        assert!(c.vertical(true));
        assert_eq!(c.cursor(), 7, "to the shorter row's end");
        assert!(c.vertical(true));
        assert_eq!(c.cursor(), 2, "the column it had in that row");
        assert!(!c.vertical(true), "no row above");
        assert!(c.vertical(false));
        assert_eq!(c.cursor(), 7);
        c.text.clear();
        c.left();
        c.insert("a");
        c.insert("b");
        assert_eq!(c.text, "ab", "text cleared from outside leaves the cursor sane");
    }

    #[test]
    fn the_cursor_shows_on_a_character_and_stays_in_view_in_long_text() {
        let t = crate::theme::Theme::builtin("dark").unwrap();
        let mut c = composer(&"word ".repeat(200));
        c.back = 900;
        let (shown, cursor) = composer_rows(&c, 40);
        assert_eq!(shown.len(), COMPOSER_ROWS);
        assert!(cursor.is_some_and(|(row, _)| row < COMPOSER_ROWS));
        let mut short = composer("abc");
        short.back = 2;
        let mut term = Terminal::new(TestBackend::new(40, 4)).unwrap();
        term.draw(|f| composer_draw(f, &t, &short)).unwrap();
        let buf = term.backend().buffer().clone();
        let cell = (0..40).map(|x| &buf[(x, 1)]).find(|cell| cell.symbol() == "b").unwrap();
        assert_eq!(cell.bg, t.accent_line, "the character under the cursor");
    }
}
