//! The two screens before a session: the splash (journal: tui-splash) and the startup page with
//! recent sessions and quick actions (journal: tui-flow). Mockups: splash-*, startup-*.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::images::{Graphic, Placement};
use crate::model::{Brand, SplashView, StartupPanel, StartupView};
use crate::theme::Theme;
use crate::view::{Drawn, Hit, Target};
use crate::widgets;

/// Cells of the big logo: 44 columns by 3 rows.
const LOGO_W: u16 = 44;
const LOGO_H: u16 = 3;

fn centred(area: Rect, width: u16) -> Rect {
    let w = width.min(area.width);
    Rect { x: area.x + (area.width - w) / 2, width: w, ..area }
}

fn row(area: Rect, y: u16) -> Rect {
    Rect { y: area.y + y, height: 1, ..area }
}

/// Logo, name, version, slogan, centred, starting at `top`. Returns the rows used.
fn brand(f: &mut Frame, area: Rect, top: u16, b: &Brand, t: &Theme, out: &mut Drawn) -> u16 {
    let logo = Rect { y: area.y + top, height: LOGO_H, ..centred(area, LOGO_W) };
    // Text fallback in the middle row of the logo's cells; the picture covers it.
    let mark = Line::from(widgets::logo_mark(t)).centered();
    f.render_widget(Paragraph::new(mark), row(logo, 1));
    out.placements.push(Placement { rect: logo, graphic: Graphic::Logo });

    let lines = [
        Line::from(Span::styled(b.name.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD))),
        Line::from(Span::styled(b.version.clone(), Style::default().fg(t.text_muted))),
        Line::from(Span::styled(b.slogan.clone(), Style::default().fg(t.text_muted))),
    ];
    for (i, l) in lines.into_iter().enumerate() {
        f.render_widget(Paragraph::new(l).centered(), row(area, top + LOGO_H + 1 + i as u16));
    }
    LOGO_H + 1 + 3
}

fn rounded(color: ratatui::style::Color, bg: ratatui::style::Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(bg))
}

pub fn splash(f: &mut Frame, area: Rect, v: &SplashView, t: &Theme, out: &mut Drawn) {
    let card_h = 2 + 2 + v.lines.len() as u16;
    let total = (LOGO_H + 1 + 3) + 1 + card_h + 1 + 3 + 1;
    let top = area.height.saturating_sub(total) / 2;
    let mut y = top + brand(f, area, top, &v.brand, t, out) + 1;

    // Wide enough for the longest line: text + 2 padding + 1 border on each side.
    let longest = v.lines.iter().chain(std::iter::once(&v.heading)).map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let card = Rect { y: area.y + y, height: card_h, ..centred(area, longest + 6) };
    let mut lines = vec![
        Line::from(Span::styled(v.heading.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD))),
        Line::default(),
    ];
    lines.extend(v.lines.iter().map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(t.text)))));
    let inner = rounded(t.border_subtle, t.surface);
    f.render_widget(Paragraph::new(lines).block(inner.padding(ratatui::widgets::Padding::horizontal(2))), card);
    y += card_h + 1;

    let button = Rect { y: area.y + y, height: 3, ..centred(area, 10) };
    f.render_widget(
        Paragraph::new(Span::styled(v.button.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)))
            .centered()
            .block(rounded(t.accent_line, t.background)),
        button,
    );
    out.hits.push(Hit { rect: button, target: Target::SplashOk });
    f.render_widget(
        Paragraph::new(Span::styled(v.hint.clone(), Style::default().fg(t.text_muted))).centered(),
        row(area, y + 3),
    );
}

/// A list of two-line entries with a rule between them; the selected one is outlined, its
/// outline drawn on the rule rows above and below it. Returns each entry's two-row rect.
fn entries(f: &mut Frame, area: Rect, count: usize, selected: Option<usize>, t: &Theme) -> Vec<Rect> {
    let mut rects = Vec::new();
    for i in 0..count {
        let sep = row(area, i as u16 * 3);
        f.render_widget(
            Paragraph::new("─".repeat(sep.width as usize)).style(Style::default().fg(t.border_subtle)),
            Rect { x: sep.x + 1, width: sep.width.saturating_sub(2), ..sep },
        );
        rects.push(Rect { y: area.y + i as u16 * 3 + 1, height: 2, ..area });
    }
    if let Some(i) = selected.filter(|i| *i < count) {
        let r = Rect { y: area.y + i as u16 * 3, height: 4, ..area };
        f.render_widget(rounded(t.accent_line, t.selection), r);
    }
    rects
}

pub fn startup(f: &mut Frame, area: Rect, v: &StartupView, t: &Theme, out: &mut Drawn) {
    // Panels are as tall as their lists (border, title, three rows per entry, closing rule);
    // what is left over goes above the logo, so the page sits low like the mockup.
    let entries_n = v.sessions.len().max(v.actions.len()) as u16;
    let panel_h = 2 + 1 + entries_n * 3 + 1;
    let brand_h = LOGO_H + 1 + 3;
    let fixed = brand_h + 1 + panel_h + 1 + 1 + 1 + 3;
    let top = area.height.saturating_sub(fixed).min(2);
    let used = brand(f, area, top, &v.brand, t, out);
    let [_, panels, _, rule, status, tip] = Layout::vertical([
        Constraint::Length(top + used + 1),
        Constraint::Length(panel_h),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(area);
    let inset = |r: Rect| Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r };
    let [left, _, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(2), Constraint::Length(50)]).areas(inset(panels));

    // Recent sessions
    let block = rounded(t.border_subtle, t.background);
    let inner = block.inner(left);
    f.render_widget(block, left);
    let title = |icon: &str, text: &str| {
        Line::from(vec![
            Span::styled(format!(" {icon}  "), Style::default().fg(t.text)),
            Span::styled(text.to_string(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        ])
    };
    f.render_widget(Paragraph::new(title("◷", "Recent sessions")), row(inner, 0));
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let sel = (v.focus == StartupPanel::Sessions).then_some(v.selected_session);
    let rects = entries(f, list, v.sessions.len(), sel, t);
    for (i, (s, r)) in v.sessions.iter().zip(&rects).enumerate() {
        let bg = if sel == Some(i) { t.selection } else { t.background };
        let r = Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..*r };
        let [icon, text, branch, age, model] = Layout::horizontal([
            Constraint::Length(3),
            Constraint::Fill(1),
            Constraint::Length(16),
            Constraint::Length(11),
            Constraint::Length(18),
        ])
        .areas(r);
        let st = Style::default().bg(bg);
        f.render_widget(Paragraph::new(Span::styled(" ▤", st.fg(t.text_muted))).style(st), icon);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(s.title.clone(), st.fg(t.text).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(s.path.clone(), st.fg(t.text_muted))),
            ])
            .style(st),
            text,
        );
        let chip = format!(" ⎇ {} ", s.branch);
        f.render_widget(Paragraph::new(Span::styled(chip, Style::default().fg(t.info).bg(t.surface_raised))).style(st), branch);
        f.render_widget(Paragraph::new(Span::styled(s.age.clone(), st.fg(t.text))).style(st), age);
        f.render_widget(Paragraph::new(Span::styled(format!("{} ", s.model), st.fg(t.text))).right_aligned().style(st), model);
        out.hits.push(Hit { rect: r, target: Target::Session(i) });
    }

    // Quick actions
    let block = rounded(t.border_subtle, t.background);
    let inner = block.inner(right);
    f.render_widget(block, right);
    f.render_widget(Paragraph::new(title("⚡", "Quick actions")), row(inner, 0));
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let sel = (v.focus == StartupPanel::Actions).then_some(v.selected_action);
    let rects = entries(f, list, v.actions.len(), sel, t);
    for (i, (a, r)) in v.actions.iter().zip(&rects).enumerate() {
        let on = sel == Some(i);
        let bg = if on { t.selection } else { t.background };
        let r = Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..*r };
        let st = Style::default().bg(bg);
        let key_style = if on {
            Style::default().fg(t.accent).bg(t.surface_raised).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text).bg(t.surface_raised)
        };
        let lines = vec![
            Line::from(vec![
                Span::styled(" ", st),
                Span::styled(format!(" {} ", a.key), key_style),
                Span::styled(format!("  {}  ", a.icon), st.fg(t.text)),
                Span::styled(a.title.clone(), st.fg(t.text).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![Span::styled("         ", st), Span::styled(a.description.clone(), st.fg(t.text_muted))]),
        ];
        f.render_widget(Paragraph::new(lines).style(st), r);
        out.hits.push(Hit { rect: r, target: Target::Action(i) });
    }

    // Status line and tip
    widgets::rule(f, inset(rule), t);
    widgets::status_line(f, status, t, &v.status);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(t.success)),
            Span::styled(format!("{}  ", v.ready), Style::default().fg(t.text)),
        ]))
        .right_aligned(),
        status,
    );
    if let Some(parts) = v.tips.get(v.tip) {
        let mut spans = vec![Span::styled(" ● Tip:  ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD))];
        for p in parts {
            if p.code {
                spans.push(Span::styled(format!(" {} ", p.text), Style::default().fg(t.text).bg(t.surface_raised).add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled(p.text.clone(), Style::default().fg(t.text)));
            }
        }
        let tip_box = inset(tip);
        let block = rounded(t.accent_line, t.background);
        let inner = block.inner(tip_box);
        f.render_widget(Paragraph::new(Line::from(spans)).block(block), tip_box);
        f.render_widget(
            Paragraph::new(Span::styled(format!("{} / {} ", v.tip + 1, v.tips.len()), Style::default().fg(t.text_muted))).right_aligned(),
            inner,
        );
    }
}
