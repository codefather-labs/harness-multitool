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

/// Cells of the big logo: 52 columns by 4 rows (the size of its hand-drawn text fallback).
const LOGO_W: u16 = 52;
const LOGO_H: u16 = 4;
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

/// `fg` laid over `bg` at `alpha` (0..=1): the tinted fills of the mockup's pills.
fn tint(fg: Color, bg: Color, alpha: f32) -> Color {
    match (fg, bg) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |a: u8, b: u8| (a as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => bg,
    }
}

fn row(area: Rect, y: u16) -> Rect {
    Rect { y: area.y + y, height: 1, ..area }
}

/// Logo, name, version, slogan, centred, starting at `top`. Returns the rows used.
/// Rows of the brand block. With pictures the logo is 3 rows (the mockup's proportion) and the
/// name a 2-row picture; without, the logo art needs 4 rows and the name is one line of text.
fn brand_h(pictures: bool) -> u16 {
    if pictures { 3 + 2 + 2 } else { LOGO_H + 1 + 2 }
}

fn brand(f: &mut Frame, area: Rect, top: u16, b: &Brand, t: &Theme, pictures: bool, out: &mut Drawn) -> u16 {
    let (logo_h, name_h) = if pictures { (3, 2) } else { (LOGO_H, 1) };
    let logo = Rect { y: area.y + top, height: logo_h, ..centred(area, LOGO_W) };
    out.placements.push(Placement { rect: logo, graphic: Graphic::Logo(crate::images::Eyes::Open) });
    // The name, two rows tall: a picture of bold text (big type is impossible in cells).
    let name = Rect { y: area.y + top + logo_h, height: name_h, ..centred(area, NAME_W) };
    out.placements.push(Placement { rect: name, graphic: Graphic::Text(b.name.clone(), Tint::Text) });

    let lines = [
        Line::from(Span::styled(b.version.clone(), Style::default().fg(t.text_muted))),
        Line::from(Span::styled(b.slogan.clone(), Style::default().fg(t.text_muted))),
    ];
    for (i, l) in lines.into_iter().enumerate() {
        f.render_widget(Paragraph::new(l).centered(), row(area, top + logo_h + name_h + i as u16));
    }
    brand_h(pictures)
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

/// A card drawn with a rounded line (╭─╮) on the colour around it. Unfilled on purpose: the
/// line runs through the middle of its cells, so a fill would stick out half a cell past the
/// line and square off the corners. Returns the inside.
fn lined(f: &mut Frame, area: Rect, line: Color, bg: Color) -> Rect {
    boxed(f, area, line, bg, bg)
}

/// A page card as the mockups draw it: light themes outline it (thin rounded line), dark themes
/// fill it without a line. `line` overrides the outline colour (e.g. the accent for the tip).
fn card(f: &mut Frame, area: Rect, t: &Theme, line: Option<Color>) -> (Rect, Color) {
    if t.dark {
        (panel(f, area, t.background, t.surface), t.surface)
    } else {
        (lined(f, area, line.unwrap_or(t.border_subtle), t.background), t.background)
    }
}

fn rounded(color: Color, bg: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .style(Style::default().bg(bg))
}

pub fn splash(f: &mut Frame, area: Rect, v: &SplashView, t: &Theme, pictures: bool, out: &mut Drawn) {
    let card_h = 2 + 2 + v.lines.len() as u16;
    let total = brand_h(pictures) + 1 + card_h + 1 + 3 + 1;
    let top = area.height.saturating_sub(total) / 2;
    let mut y = top + brand(f, area, top, &v.brand, t, pictures, out) + 1;

    // Wide enough for the longest line: text + 2 padding + 1 border on each side.
    let longest = v.lines.iter().chain(std::iter::once(&v.heading)).map(|l| l.chars().count()).max().unwrap_or(0) as u16;
    let card = Rect { y: area.y + y, height: card_h, ..centred(area, longest + 6) };
    let mut lines = vec![
        // The light mockup writes the heading in the accent, the dark one in the text colour.
        Line::from(Span::styled(v.heading.clone(), Style::default().fg(if t.dark { t.text } else { t.accent }).add_modifier(Modifier::BOLD))),
        Line::default(),
    ];
    lines.extend(v.lines.iter().map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(t.text)))));
    let card_line = if t.dark { t.border_subtle } else { t.accent_line };
    let inside = lined(f, card, card_line, t.background);
    let text = Rect { x: inside.x + 2, width: inside.width.saturating_sub(4), ..inside };
    f.render_widget(Paragraph::new(lines).style(Style::default().bg(t.background)), text);
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
fn entries(f: &mut Frame, area: Rect, count: usize, selected: Option<usize>, t: &Theme, fill: Color) -> Vec<Rect> {
    let mut rects = Vec::new();
    for i in 0..count {
        let sep = row(area, i as u16 * 3);
        f.render_widget(
            Paragraph::new("─".repeat(sep.width as usize)).style(Style::default().fg(t.border_subtle).bg(fill)),
            Rect { x: sep.x + 1, width: sep.width.saturating_sub(2), ..sep },
        );
        rects.push(Rect { y: area.y + i as u16 * 3 + 1, height: 2, ..area });
    }
    if let Some(i) = selected.filter(|i| *i < count) {
        let r = Rect { y: area.y + i as u16 * 3, height: 4, ..area };
        boxed(f, r, t.accent_line, fill, t.selection);
    }
    rects
}

pub fn startup(f: &mut Frame, area: Rect, v: &StartupView, t: &Theme, pictures: bool, out: &mut Drawn) {
    // Bottom block as in the mockup: rule, status line, rule, tip card. Above it the brand and
    // the two panels, as one group centred in the space left (a tall terminal gets even
    // margins instead of a hole between the panels and the status line).
    let [top_area, rule1, status, rule2, tip] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
    ])
    .areas(area);
    let entries_n = v.sessions.len().max(v.actions.len()) as u16;
    let panel_h = 2 + 1 + entries_n * 3 + 1;
    let group = brand_h(pictures) + 1 + panel_h;
    let top = top_area.height.saturating_sub(group + 1) / 2 + 1;
    let top = top.min(top_area.height.saturating_sub(group));
    let used = brand(f, top_area, top, &v.brand, t, pictures, out);
    let panels = Rect { y: top_area.y + top + used + 1, height: panel_h.min(top_area.height.saturating_sub(top + used + 1)), ..top_area };
    let inset = |r: Rect| Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..r };

    // Column widths from the content. Sessions: icon, title/path, branch chip, age, model.
    let w = |it: &mut dyn Iterator<Item = usize>| it.max().unwrap_or(0) as u16;
    let text_w = w(&mut v.sessions.iter().map(|s| s.title.chars().count().max(s.path.chars().count())));
    // Branch pill: icon (2 cells, from the pill's first cell), a space, the name, a space.
    let chip_w = w(&mut v.sessions.iter().map(|s| s.branch.chars().count())) + 4;
    let age_w = w(&mut v.sessions.iter().map(|s| s.age.chars().count()));
    let model_w = w(&mut v.sessions.iter().map(|s| s.model.chars().count()));
    // Quick actions: card edges (2), entry insets (2), the 11-cell indent (key chip, icon), the
    // text and one cell of air before the selection outline. Key chip at 1..=3, icon at 5..=7.
    let act_text = w(&mut v.actions.iter().map(|a| a.description.chars().count().max(a.title.chars().count())));
    let actions_need = 2 + 2 + 9 + act_text + 1;
    let sessions_need = 4 + text_w + 3 + chip_w + 3 + age_w + 3 + model_w + 2 + 4;
    let room = inset(panels).width.saturating_sub(2);
    let actions_w = actions_need.min(room * 2 / 5).max(room.saturating_sub(sessions_need).min(room * 2 / 5).min(actions_need));
    let [left, _, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(1), Constraint::Length(actions_w)]).areas(inset(panels));

    let heading = |text: &str| {
        Line::from(vec![
            Span::raw("     "),
            Span::styled(text.to_string(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        ])
    };

    // Recent sessions
    let (inner, fill) = card(f, left, t, None);
    f.render_widget(Paragraph::new(heading("Recent sessions")).style(Style::default().bg(fill)), row(inner, 0));
    icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Clock, Tint::Text);
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    // Both panels show their selection, as in the mockup; the focused one takes the keys.
    let sel = Some(v.selected_session);
    let rects = entries(f, list, v.sessions.len(), sel, t, fill);
    for (i, (s, r)) in v.sessions.iter().zip(&rects).enumerate() {
        let bg = if sel == Some(i) { t.selection } else { fill };
        let r = Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..*r };
        // Spare width is shared by the three gaps between the columns, like the mockup's
        // evenly spread columns; a narrow panel shortens the title column instead.
        let fixed = 4 + chip_w + age_w + model_w + 1;
        let spare = r.width.saturating_sub(fixed + text_w);
        let gap = (spare / 3).clamp(1, 12);
        let text_col = r.width.saturating_sub(fixed + gap * 3).min(text_w + spare.saturating_sub(gap * 3));
        let [icon_r, text, _, branch, _, age, _, model] = Layout::horizontal([
            Constraint::Length(4),
            Constraint::Length(text_col),
            Constraint::Length(gap),
            Constraint::Length(chip_w),
            Constraint::Length(gap),
            Constraint::Length(age_w),
            Constraint::Length(gap),
            Constraint::Length(model_w),
        ])
        .areas(r);
        let st = Style::default().bg(bg);
        f.render_widget(Paragraph::new("").style(st), r);
        big_icon(out, icon_r.x, icon_r.y, Icon::File, Tint::Muted);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(fit(&s.title, text.width), st.fg(t.text).add_modifier(Modifier::BOLD))),
                Line::from(Span::styled(fit(&s.path, text.width), st.fg(t.text_muted))),
            ])
            .style(st),
            text,
        );
        // The chip is as wide as its text, on the middle of the entry like the mockup's pill.
        let chip = format!("   {} ", s.branch);
        let cw = (chip.chars().count() as u16).min(branch.width);
        let chip_r = Rect { y: branch.y, height: 1, width: cw, ..branch };
        let pill = tint(t.info, bg, 0.16);
        f.render_widget(Paragraph::new(Span::styled(chip, Style::default().fg(t.info).bg(pill).add_modifier(Modifier::BOLD))), chip_r);
        icon(out, chip_r, Icon::Branch, Tint::Info);
        f.render_widget(Paragraph::new(Span::styled(s.age.clone(), st.fg(t.text))).style(st), Rect { height: 1, ..age });
        f.render_widget(Paragraph::new(Span::styled(s.model.clone(), st.fg(t.text))).style(st), Rect { height: 1, ..model });
        out.hits.push(Hit { rect: r, target: Target::Session(i) });
    }

    // Quick actions
    let (inner, fill) = card(f, right, t, None);
    f.render_widget(Paragraph::new(heading("Quick actions")).style(Style::default().bg(fill)), row(inner, 0));
    icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Bolt, Tint::Accent);
    let list = Rect { y: inner.y + 1, height: inner.height.saturating_sub(1), ..inner };
    let sel = Some(v.selected_action);
    let rects = entries(f, list, v.actions.len(), sel, t, fill);
    for (i, (a, r)) in v.actions.iter().zip(&rects).enumerate() {
        let on = sel == Some(i);
        let bg = if on { t.selection } else { fill };
        let r = Rect { x: r.x + 1, width: r.width.saturating_sub(2), ..*r };
        let st = Style::default().bg(bg);
        let key_style = if on {
            Style::default().fg(t.accent).bg(tint(t.accent, t.selection, 0.22)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text).bg(t.surface_raised).add_modifier(Modifier::BOLD)
        };
        let indent = 9u16;
        let lines = vec![
            Line::from(vec![
                Span::styled(" ", st),
                Span::styled(format!(" {} ", a.key), key_style),
                Span::styled(" ".repeat(indent as usize - 4), st),
                Span::styled(a.title.clone(), st.fg(t.text).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled(" ".repeat(indent as usize), st),
                Span::styled(fit(&a.description, r.width.saturating_sub(indent)), st.fg(t.text_muted)),
            ]),
        ];
        f.render_widget(Paragraph::new(lines).style(st), r);
        big_icon(out, r.x + 5, r.y, a.icon, Tint::Text);
        out.hits.push(Hit { rect: r, target: Target::Action(i) });
    }

    // Status line between two rules, then the tip
    widgets::rule(f, inset(rule1), t);
    widgets::rule(f, inset(rule2), t);
    let status = inset(status);
    // The THEME field names the theme being drawn, whatever the state says (F2 changes it).
    let mut fields = v.status.clone();
    for (k, val) in &mut fields.fields {
        if k == "THEME" {
            *val = t.id.clone();
        }
    }
    widgets::status_line(f, status, t, &fields);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("● ", Style::default().fg(t.success)),
            Span::styled(format!("{} ", v.ready), Style::default().fg(t.text)),
        ]))
        .right_aligned(),
        status,
    );
    if let Some(parts) = v.tips.get(v.tip) {
        let (inner, fill) = card(f, inset(tip), t, Some(t.accent_line));
        let mut spans = vec![Span::raw("    "), Span::styled("Tip:  ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD))];
        for p in parts {
            if p.code {
                spans.push(Span::styled(format!(" {} ", p.text), Style::default().fg(t.text).bg(t.surface_raised).add_modifier(Modifier::BOLD)));
            } else {
                spans.push(Span::styled(p.text.clone(), Style::default().fg(t.text)));
            }
        }
        f.render_widget(Paragraph::new(Line::from(spans)).style(Style::default().bg(fill)), inner);
        icon(out, Rect { x: inner.x + 1, ..row(inner, 0) }, Icon::Info, Tint::Accent);
        f.render_widget(
            Paragraph::new(Span::styled(format!("{} / {} ", v.tip + 1, v.tips.len()), Style::default().fg(t.text_muted))).right_aligned(),
            inner,
        );
    }
}
