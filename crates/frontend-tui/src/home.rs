//! The two screens before a session: the splash (journal: tui-splash) and the startup page with
//! recent sessions and quick actions (journal: tui-flow). Mockups: splash-*, startup-*.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::images::{Graphic, Icon, Placement, Tint};
use crate::model::{Brand, SplashView, StartupView};
use crate::theme::Theme;
use crate::view::{Drawn, Hit, Target};
use crate::widgets;

/// Cells of the big logo: 44 columns by 3 rows.
const LOGO_W: u16 = 44;
const LOGO_H: u16 = 3;
/// Cells of the big product name: 46 columns by 2 rows.
const NAME_W: u16 = 46;

fn centred(area: Rect, width: u16) -> Rect {
    let w = width.min(area.width);
    Rect { x: area.x + (area.width - w) / 2, width: w, ..area }
}

/// `s` cut to `width` characters with an ellipsis, so long names end cleanly.
fn fit(s: &str, width: u16) -> String {
    let w = width as usize;
    if s.chars().count() <= w {
        return s.to_string();
    }
    let mut out: String = s.chars().take(w.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn row(area: Rect, y: u16) -> Rect {
    Rect { y: area.y + y, height: 1, ..area }
}

/// Logo, name, version, slogan, centred, starting at `top`. Returns the rows used.
fn brand(f: &mut Frame, area: Rect, top: u16, b: &Brand, t: &Theme, out: &mut Drawn) -> u16 {
    let logo = Rect { y: area.y + top, height: LOGO_H, ..centred(area, LOGO_W) };
    out.placements.push(Placement { rect: logo, graphic: Graphic::Logo });
    // The name, two rows tall: a picture of bold text (big type is impossible in cells).
    let name = Rect { y: area.y + top + LOGO_H, height: 2, ..centred(area, NAME_W) };
    out.placements.push(Placement { rect: name, graphic: Graphic::Text(b.name.clone(), Tint::Text) });

    let lines = [
        Line::from(Span::styled(b.version.clone(), Style::default().fg(t.text_muted))),
        Line::from(Span::styled(b.slogan.clone(), Style::default().fg(t.text_muted))),
    ];
    for (i, l) in lines.into_iter().enumerate() {
        f.render_widget(Paragraph::new(l).centered(), row(area, top + LOGO_H + 2 + i as u16));
    }
    LOGO_H + 2 + 2
}

/// A two-cell icon at the start of `area`, one row tall.
fn icon(out: &mut Drawn, area: Rect, icon: Icon, tint: Tint) {
    out.placements.push(Placement { rect: Rect { width: 2, height: 1, ..area }, graphic: Graphic::Icon(icon, tint) });
}

/// A large icon spanning both rows of a list entry: three cells by two rows.
fn big_icon(out: &mut Drawn, x: u16, y: u16, icon: Icon, tint: Tint) {
    out.placements.push(Placement { rect: Rect { x, y, width: 3, height: 2 }, graphic: Graphic::Icon(icon, tint) });
}

/// A rounded frame whose border cells keep the colour around it (`outer`) and whose inside is
/// filled with `inner`, so a panel's fill never shows outside its line. Returns the inside.
fn boxed(f: &mut Frame, area: Rect, line: Color, outer: Color, inner: Color) -> Rect {
    let block = rounded(line, outer);
    let inside = block.inner(area);
    f.render_widget(block, area);
    widgets::fill(f, inside, inner);
    inside
}

/// A filled card without a line: half blocks on the edges and quarter blocks in the corners,
/// so the fill starts right at the card's edge and the corners read as rounded. Returns the
/// inside (the area less one cell on each side).
fn panel(f: &mut Frame, area: Rect, outer: Color, inner: Color) -> Rect {
    let buf = f.buffer_mut();
    let a = area.intersection(buf.area);
    if a.width < 2 || a.height < 2 {
        return a;
    }
    let (r, b) = (a.x + a.width - 1, a.y + a.height - 1);
    for y in a.y..=b {
        for x in a.x..=r {
            let sym = match (x == a.x, x == r, y == a.y, y == b) {
                (true, _, true, _) => "▗",
                (_, true, true, _) => "▖",
                (true, _, _, true) => "▝",
                (_, true, _, true) => "▘",
                (_, _, true, _) => "▄",
                (_, _, _, true) => "▀",
                (true, _, _, _) => "▐",
                (_, true, _, _) => "▌",
                _ => " ",
            };
            let cell = &mut buf[(x, y)];
            if sym == " " {
                cell.set_symbol(" ").set_bg(inner);
            } else {
                cell.set_symbol(sym).set_fg(inner).set_bg(outer);
            }
        }
    }
    Rect { x: a.x + 1, y: a.y + 1, width: a.width - 2, height: a.height - 2 }
}

fn rounded(color: Color, bg: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(bg))
}

pub fn splash(f: &mut Frame, area: Rect, v: &SplashView, t: &Theme, out: &mut Drawn) {
    let card_h = 2 + 2 + v.lines.len() as u16;
    let total = (LOGO_H + 2 + 2) + 1 + card_h + 1 + 3 + 1;
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
    let inside = panel(f, card, t.background, t.surface);
    let text = Rect { x: inside.x + 2, width: inside.width.saturating_sub(4), ..inside };
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.surface)), text);
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
            Paragraph::new("─".repeat(sep.width as usize)).style(Style::default().fg(t.border_subtle).bg(t.surface)),
            Rect { x: sep.x + 1, width: sep.width.saturating_sub(2), ..sep },
        );
        rects.push(Rect { y: area.y + i as u16 * 3 + 1, height: 2, ..area });
    }
    if let Some(i) = selected.filter(|i| *i < count) {
        let r = Rect { y: area.y + i as u16 * 3, height: 4, ..area };
        boxed(f, r, t.accent_line, t.surface, t.selection);
    }
    rects
}

pub fn startup(f: &mut Frame, area: Rect, v: &StartupView, t: &Theme, out: &mut Drawn) {
    // Panels are as tall as their lists (border, title, three rows per entry, closing rule);
    // what is left over goes above the logo, so the page sits low like the mockup.
    let entries_n = v.sessions.len().max(v.actions.len()) as u16;
    let panel_h = 2 + 1 + entries_n * 3 + 1;
    let brand_h = LOGO_H + 2 + 2;
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
    // Quick actions are as wide as their longest description needs (key chip, icon, text,
    // borders), but never more than 40% of the screen; the sessions take the rest.
    let longest = v.actions.iter().map(|a| a.description.chars().count().max(a.title.chars().count())).max().unwrap_or(0) as u16;
    let actions_w = (longest + 12 + 4).min(area.width * 2 / 5);
    let [left, _, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(2), Constraint::Length(actions_w)]).areas(inset(panels));

    // Recent sessions
    let inner = panel(f, left, t.background, t.surface);
    let title = |text: &str| {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(text.to_string(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        ])
    };
    f.render_widget(Paragraph::new(title("Recent sessions")), row(inner, 0));
    icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Clock, Tint::Text);
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    // Both panels show their selection, as in the mockup; the focused one takes the keys.
    let sel = Some(v.selected_session);
    let rects = entries(f, list, v.sessions.len(), sel, t);
    for (i, (s, r)) in v.sessions.iter().zip(&rects).enumerate() {
        let bg = if sel == Some(i) { t.selection } else { t.surface };
        let r = Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..*r };
        let [icon_r, text, branch, age, model] = Layout::horizontal([
            Constraint::Length(5),
            Constraint::Fill(1),
            Constraint::Length(18),
            Constraint::Length(12),
            Constraint::Length(18),
        ])
        .areas(r);
        let st = Style::default().bg(bg);
        f.render_widget(Paragraph::new("").style(st), icon_r);
        big_icon(out, icon_r.x + 1, icon_r.y, Icon::File, Tint::Muted);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(fit(&s.title, text.width), st.fg(t.text).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(fit(&s.path, text.width), st.fg(t.text_muted))),
            ])
            .style(st),
            text,
        );
        // The chip is as wide as its text, centred on the first row of the entry.
        let chip = format!("    {} ", s.branch);
        let chip_w = (chip.chars().count() as u16).min(branch.width);
        f.render_widget(Paragraph::new("").style(st), branch);
        let chip_r = Rect { y: branch.y, height: 1, width: chip_w, ..branch };
        f.render_widget(Paragraph::new(Span::styled(chip, Style::default().fg(t.info).bg(t.surface_raised).add_modifier(Modifier::BOLD))), chip_r);
        icon(out, Rect { x: chip_r.x + 1, ..chip_r }, Icon::Branch, Tint::Info);
        f.render_widget(Paragraph::new(Span::styled(s.age.clone(), st.fg(t.text))).style(st), age);
        f.render_widget(Paragraph::new(Span::styled(format!("{} ", s.model), st.fg(t.text))).right_aligned().style(st), model);
        out.hits.push(Hit { rect: r, target: Target::Session(i) });
    }

    // Quick actions
    let inner = panel(f, right, t.background, t.surface);
    f.render_widget(Paragraph::new(title("Quick actions")), row(inner, 0));
    icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Bolt, Tint::Accent);
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let sel = Some(v.selected_action);
    let rects = entries(f, list, v.actions.len(), sel, t);
    for (i, (a, r)) in v.actions.iter().zip(&rects).enumerate() {
        let on = sel == Some(i);
        let bg = if on { t.selection } else { t.surface };
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
                Span::styled("        ", st),
                Span::styled(a.title.clone(), st.fg(t.text).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![Span::styled("            ", st), Span::styled(fit(&a.description, r.width.saturating_sub(12)), st.fg(t.text_muted))]),
        ];
        f.render_widget(Paragraph::new(lines).style(st), r);
        big_icon(out, r.x + 6, r.y, a.icon, Tint::Text);
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
        let mut spans = vec![Span::raw("    "), Span::styled("Tip:  ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD))];
        for p in parts {
            if p.code {
                spans.push(Span::styled(format!(" {} ", p.text), Style::default().fg(t.text).bg(t.surface_raised).add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled(p.text.clone(), Style::default().fg(t.text)));
            }
        }
        let tip_box = inset(tip);
        let inner = panel(f, tip_box, t.background, t.surface);
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(t.surface)), inner);
        icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Info, Tint::Accent);
        f.render_widget(
            Paragraph::new(Span::styled(format!("{} / {} ", v.tip + 1, v.tips.len()), Style::default().fg(t.text_muted))).right_aligned(),
            inner,
        );
    }
}
