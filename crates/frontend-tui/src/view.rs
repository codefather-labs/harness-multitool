//! Draws an `AppState`. The only drawing entry point of the terminal frontend: the live CLI and
//! the snapshot tool both call `draw`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::{AppState, DiffLine, Item, Mode, Role, SessionView, Tone, ToolBody, ToolCall, ToolState};
use crate::theme::Theme;
use crate::widgets;

pub fn draw(f: &mut Frame, s: &AppState, t: &Theme) {
    let area = f.area();
    widgets::fill(f, area, t.background);
    let [head, rule, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    widgets::header(f, head, t, &s.header);
    widgets::rule(f, rule, t);
    match &s.mode {
        Mode::Session(v) => session(f, body, v, t),
    }
}

fn inset(r: Rect) -> Rect {
    Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r }
}

/// Session chrome (journal: one session chrome): transcript, rule, status line, composer with
/// key hints inside, and the Settings button to its right.
fn session(f: &mut Frame, area: Rect, v: &SessionView, t: &Theme) {
    let [transcript, rule, status, bottom] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(4),
    ])
    .areas(area);
    draw_transcript(f, inset(transcript), &v.transcript, t);
    widgets::rule(f, rule, t);
    widgets::status_line(f, status, t, &v.status);
    let [input, _, button] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(14)]).areas(inset(bottom));
    widgets::composer(f, input, t, &v.composer);
    widgets::button(f, button, t, "⚙ Settings", "Ctrl+,");
}

/// Height in rows an item takes: label row, body rows, one bottom padding row.
fn height(item: &Item) -> u16 {
    match item {
        Item::Welcome { .. } => 4,
        Item::Tips { lines, .. } => 2 + lines.len() as u16,
        Item::Turn { lines, .. } => 2 + lines.len() as u16,
        Item::Tool(c) => match (&c.body, c.expanded) {
            (ToolBody::Diff(d), true) => 1 + 1 + d.lines.len() as u16 + 1,
            _ => 1,
        },
    }
}

fn draw_transcript(f: &mut Frame, area: Rect, items: &[Item], t: &Theme) {
    let bottom = area.y + area.height;
    let mut y = area.y;
    for item in items {
        let h = height(item);
        if y + h > bottom {
            break;
        }
        let r = Rect { y, height: h, ..area };
        match item {
            Item::Welcome { title, tagline } => widgets::card(
                f,
                r,
                t,
                vec![Span::styled("WELCOME", label(t))],
                vec![
                    Line::from(Span::styled(title.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD))),
                    Line::from(Span::styled(tagline.clone(), Style::default().fg(t.text_muted))),
                ],
            ),
            Item::Tips { label: l, lines } => widgets::card(
                f,
                r,
                t,
                vec![Span::styled(l.clone(), label(t))],
                lines
                    .iter()
                    .map(|l| {
                        Line::from(vec![
                            Span::styled(" • ", Style::default().fg(t.text_muted)),
                            Span::styled(l.clone(), Style::default().fg(t.text)),
                        ])
                    })
                    .collect(),
            ),
            Item::Turn { role, time, lines } => {
                let name = match role {
                    Role::User => "USER",
                    Role::Assistant => "ASSISTANT",
                };
                widgets::card(
                    f,
                    r,
                    t,
                    vec![Span::styled(name, label(t)), Span::styled(format!("  {time}"), Style::default().fg(t.text_muted))],
                    lines.iter().map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(t.text)))).collect(),
                )
            }
            Item::Tool(c) => tool(f, r, c, t),
        }
        y += h + 1;
    }
}

fn label(t: &Theme) -> Style {
    Style::default().fg(t.accent).add_modifier(Modifier::BOLD)
}

/// A tool call: one row (chevron, status, name, arguments … outcome, duration) and, when
/// expanded, its body below on the same surface.
fn tool(f: &mut Frame, r: Rect, c: &ToolCall, t: &Theme) {
    widgets::fill(f, r, t.surface);
    let row = Rect { height: 1, ..r };
    let chevron = if c.expanded { "▾" } else { "▸" };
    let (mark, mark_color) = match c.state {
        ToolState::Done => ("✔", t.success),
        ToolState::Running => ("◌", t.accent),
        ToolState::Failed => ("✘", t.error),
    };
    let mut left = vec![
        Span::styled(format!("  {chevron}  "), Style::default().fg(t.text_muted)),
        Span::styled(mark, Style::default().fg(mark_color)),
        Span::raw("  "),
        Span::styled(c.name.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
    ];
    if let Some(s) = &c.args_strong {
        left.push(Span::styled(format!(" {s}"), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
    }
    left.push(Span::styled(format!("  {}", c.args), Style::default().fg(t.text_muted)));
    f.render_widget(Paragraph::new(Line::from(left)).style(Style::default().bg(t.surface)), row);

    let tone = match c.outcome_tone {
        Tone::Success => t.success,
        Tone::Neutral => t.text,
        Tone::Error => t.error,
    };
    let mut right = vec![Span::styled(c.outcome.clone(), Style::default().fg(tone))];
    if let Some((add, del)) = c.diffstat {
        right.push(Span::raw("   "));
        right.push(Span::styled(format!("+{add}"), Style::default().fg(t.success)));
        right.push(Span::styled(" / ", Style::default().fg(t.text_muted)));
        right.push(Span::styled(format!("-{del}"), Style::default().fg(t.error)));
    }
    right.push(Span::styled(format!("   {:>5}  ", c.duration), Style::default().fg(t.text)));
    f.render_widget(Paragraph::new(Line::from(right)).right_aligned(), row);

    if !c.expanded {
        return;
    }
    if let ToolBody::Diff(d) = &c.body {
        let x = r.x + 3;
        let w = r.width.saturating_sub(5);
        let hunk = Rect { x, y: r.y + 1, width: w, height: 1 };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(d.hunk.clone(), Style::default().fg(t.text_muted)),
                Span::styled(format!("    {}", d.context), Style::default().fg(t.text_muted)),
            ]))
            .style(Style::default().bg(t.surface)),
            hunk,
        );
        for (i, l) in d.lines.iter().enumerate() {
            let (sign, text, bg, sign_fg) = match l {
                DiffLine::Removed(s) => ("-", s, t.diff_del_bg, t.error),
                DiffLine::Added(s) => ("+", s, t.diff_add_bg, t.success),
                DiffLine::Context(s) => (" ", s, t.surface, t.text_muted),
            };
            let line = Rect { x, y: r.y + 2 + i as u16, width: w, height: 1 };
            f.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!(" {sign}     "), Style::default().fg(sign_fg)),
                    Span::styled(text.clone(), Style::default().fg(t.text)),
                ]))
                .style(Style::default().bg(bg)),
                line,
            );
        }
    }
}
