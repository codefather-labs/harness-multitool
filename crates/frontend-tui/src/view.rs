//! Draws an `AppState`. The only drawing entry point of the terminal frontend: the live CLI and
//! the snapshot tool both call `draw`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::model::{AppState, Item, Mode, SessionView};
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

fn session(f: &mut Frame, area: Rect, v: &SessionView, t: &Theme) {
    let [transcript, rule, input, foot] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);
    draw_transcript(f, inset(transcript), &v.transcript, t);
    widgets::rule(f, inset(rule), t);
    widgets::composer(f, inset(input), t, &v.composer);
    widgets::hints(f, foot, t, &v.hints);
}

/// Height in rows an item takes, including its own padding.
fn height(item: &Item) -> u16 {
    match item {
        Item::Welcome { .. } => 5,
        Item::Tips { lines, .. } => 3 + lines.len() as u16,
    }
}

fn draw_transcript(f: &mut Frame, area: Rect, items: &[Item], t: &Theme) {
    let mut y = area.y + 1;
    for item in items {
        let h = height(item);
        if y + h > area.y + area.height {
            break;
        }
        let r = Rect { y, height: h, ..area };
        match item {
            Item::Welcome { title, tagline } => widgets::card(
                f,
                r,
                t,
                "WELCOME",
                vec![
                    Line::from(Span::styled(title.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD))),
                    Line::from(Span::styled(tagline.clone(), Style::default().fg(t.text_muted))),
                ],
            ),
            Item::Tips { label, lines } => widgets::card(
                f,
                r,
                t,
                label,
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
        }
        y += h + 1;
    }
}
