//! The transcript of a session: every kind of item drawn as its mockup shows it.
//!
//! Each item is drawn into a buffer of its own, as tall as it needs at the transcript's width;
//! the transcript then shows the bottom of that stack (or a scrolled part of it). So an item's
//! height is always exactly what was drawn, and a view can start mid-item.
//!
//! Mockups: main-session-* (turns, tool rows, diff), inbound-messages-* (inbound cards),
//! tool-calls-* (tool cards), journal-mindmap-events-* (journal and mind-map events).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::images::{Graphic, Icon, Placement, Tint};
use crate::model::{
    DiffLine, FlowchartEvent, Inbound, InboundKind, Item, JournalEvent, MapCreated, MapTool, MindMapEvent, Role, Tone, ToolBody, ToolCall, ToolIcon, ToolState,
};
use crate::theme::Theme;

/// Where a card's text starts: the stripe, then two cells of air.
const CX: u16 = 3;
/// Rows between two items: none; every item ends with a half-row separator instead (see
/// `separate`), so cards sit as close as the mockups draw them.
pub const GAP: u16 = 0;
/// The tallest an item may draw.
const MAX_H: u16 = 400;

/// An item drawn on its own: its cells and its pictures (relative to the buffer's origin).
pub struct Drawn {
    pub buf: Buffer,
    pub placements: Vec<Placement>,
}

/// `text` broken into lines of at most `width` columns, at spaces where possible.
pub fn wrap(text: &str, width: u16) -> Vec<String> {
    let w = width.max(1) as usize;
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let (cl, wl) = (cur.chars().count(), word.chars().count());
            if cl > 0 && cl + 1 + wl > w {
                out.push(std::mem::take(&mut cur));
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
            while cur.chars().count() > w {
                let head: String = cur.chars().take(w).collect();
                cur = cur.chars().skip(w).collect();
                out.push(head);
            }
        }
        out.push(cur);
    }
    out
}

/// `fg` laid over `bg` at `alpha`: the tinted fills of pills and highlights.
pub fn tint(fg: Color, bg: Color, alpha: f32) -> Color {
    match (fg, bg) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |a: u8, b: u8| (a as f32 * alpha + b as f32 * (1.0 - alpha)).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => bg,
    }
}

/// A drawing surface for one item.
struct Canvas<'a> {
    buf: Buffer,
    placements: Vec<Placement>,
    width: u16,
    t: &'a Theme,
}

impl<'a> Canvas<'a> {
    fn new(width: u16, t: &'a Theme) -> Canvas<'a> {
        Canvas { buf: Buffer::empty(Rect::new(0, 0, width, 4)), placements: Vec::new(), width, t }
    }

    /// Makes the buffer at least `h` rows tall (it starts small and doubles: most items are a
    /// few rows, and a frame redraws every visible item).
    fn grow(&mut self, h: u16) {
        let h = h.min(MAX_H);
        let cur = self.buf.area.height;
        if h > cur {
            self.buf.resize(Rect::new(0, 0, self.width, h.max(cur * 2).min(MAX_H)));
        }
    }

    fn fill(&mut self, r: Rect, bg: Color) {
        self.grow(r.y + r.height);
        let r = r.intersection(self.buf.area);
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                self.buf[(x, y)].set_symbol(" ").set_bg(bg);
            }
        }
    }

    /// Spans at (x, y), cut at `x + w`, on `bg`.
    fn line(&mut self, x: u16, y: u16, w: u16, spans: Vec<Span<'static>>, bg: Color) {
        if y >= MAX_H {
            return;
        }
        self.grow(y + 1);
        let line = Line::from(spans).style(Style::default().bg(bg));
        self.buf.set_line(x, y, &line, w);
    }

    /// Spans ending at `right` (exclusive). Returns where they start.
    fn line_right(&mut self, right: u16, y: u16, spans: Vec<Span<'static>>, bg: Color) -> u16 {
        let w: u16 = spans.iter().map(|s| s.content.chars().count() as u16).sum();
        let x = right.saturating_sub(w);
        self.line(x, y, w, spans, bg);
        x
    }

    /// A pill: half-block caps in `fill` on `bg`, `text` in `fg` on `fill`. Returns its width.
    fn pill(&mut self, x: u16, y: u16, text: &str, fg: Color, fill: Color, bg: Color) -> u16 {
        let spans = pill_spans(text, fg, fill, bg);
        let w = text.chars().count() as u16 + 2;
        self.line(x, y, w, spans, bg);
        w
    }

    fn icon(&mut self, x: u16, y: u16, w: u16, h: u16, icon: Icon, tint: Tint) {
        self.grow(y + h);
        let p = Placement { rect: Rect::new(x, y, w, h), graphic: Graphic::Icon(icon, tint) };
        crate::images::fallback(&mut self.buf, &p, self.t);
        self.placements.push(p);
    }

    /// A card's stripe: a half block in `color` down the first column.
    fn stripe(&mut self, y: u16, h: u16, color: Color, bg: Color) {
        self.grow(y + h);
        for row in y..y + h {
            self.buf[(0, row)].set_symbol("▌").set_fg(color).set_bg(bg);
        }
    }

    /// A rounded frame in `line` around `r`, on `bg` inside and out.
    fn frame(&mut self, r: Rect, line: Color, bg: Color) {
        self.grow(r.y + r.height);
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(line))
            .style(Style::default().bg(bg))
            .render(r, &mut self.buf);
    }

    /// Another item drawn into this one at (x, y). Returns its height.
    fn embed(&mut self, x: u16, y: u16, d: Drawn) -> u16 {
        let h = d.buf.area.height;
        self.grow(y + h);
        for row in 0..h {
            for col in 0..d.buf.area.width {
                if y + row < MAX_H && x + col < self.width {
                    self.buf[(x + col, y + row)] = d.buf[(col, row)].clone();
                }
            }
        }
        for mut p in d.placements {
            p.rect.x += x;
            p.rect.y += y;
            self.placements.push(p);
        }
        h
    }

    fn finish(mut self, height: u16) -> Drawn {
        let h = height.clamp(1, MAX_H);
        self.grow(h);
        let mut buf = Buffer::empty(Rect::new(0, 0, self.width, h));
        for y in 0..h {
            for x in 0..self.width {
                buf[(x, y)] = self.buf[(x, y)].clone();
            }
        }
        Drawn { buf, placements: self.placements.into_iter().filter(|p| p.rect.y + p.rect.height <= h).collect() }
    }
}

pub fn pill_spans(text: &str, fg: Color, fill: Color, bg: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled("▐", Style::default().fg(fill).bg(bg)),
        Span::styled(text.to_string(), Style::default().fg(fg).bg(fill).add_modifier(Modifier::BOLD)),
        Span::styled("▌", Style::default().fg(fill).bg(bg)),
    ]
}

fn bold(c: Color) -> Style {
    Style::default().fg(c).add_modifier(Modifier::BOLD)
}

fn plain(c: Color) -> Style {
    Style::default().fg(c)
}

/// Draws one item `width` columns wide. `generating` marks the newest assistant turn as busy.
pub fn item(it: &Item, width: u16, t: &Theme, generating: bool) -> Drawn {
    match it {
        Item::Welcome { title, tagline } => card(width, t, t.accent_line, vec![Span::styled("WELCOME", bold(t.accent))], vec![
            vec![Span::styled(title.clone(), bold(t.text))],
            vec![Span::styled(tagline.clone(), plain(t.text_muted))],
        ]),
        Item::Tips { label, lines } => card(
            width,
            t,
            t.accent_line,
            vec![Span::styled(label.clone(), bold(t.accent))],
            lines.iter().map(|l| vec![Span::styled(" • ", plain(t.text_muted)), Span::styled(l.clone(), plain(t.text))]).collect(),
        ),
        Item::Turn { role, time, lines, attached } => turn(width, t, *role, time, lines, attached, generating),
        Item::Tool(c) => {
            let mut cv = Canvas::new(width, t);
            let h = tool(&mut cv, 0, 0, width, c, t.surface);
            cv.finish(h)
        }
        Item::Inbound(m) => inbound(width, t, m),
        Item::Journal(j) => {
            let mut cv = Canvas::new(width, t);
            let h = journal(&mut cv, 0, 0, width, j, t.background);
            cv.finish(h)
        }
        Item::MapCreated(m) => {
            let mut cv = Canvas::new(width, t);
            let h = map_created(&mut cv, 0, 0, width, m, t.background);
            cv.finish(h)
        }
        Item::MindMap(e) => {
            let mut cv = Canvas::new(width, t);
            let h = mind_map(&mut cv, 0, 0, width, e, t.background);
            cv.finish(h)
        }
        Item::Flowchart(e) => {
            let mut cv = Canvas::new(width, t);
            let h = flowchart(&mut cv, 0, 0, width, e, t.background);
            cv.finish(h)
        }
        Item::Hook(e) => {
            let mut cv = Canvas::new(width, t);
            let h = hook(&mut cv, 0, 0, width, e, t.background);
            cv.finish(h)
        }
    }
}

/// Hooks that ran on a loop event: a framed card, `HOOK event`, one row per script with its
/// outcome; context a hook added is shown under it, a veto in the warning colour and a failure
/// in the error colour (and the frame takes the colour of the worst outcome).
fn hook(cv: &mut Canvas, x: u16, y: u16, w: u16, e: &crate::model::HookEvent, bg: Color) -> u16 {
    use crate::model::HookOutcome as O;
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let frame = if e.runs.iter().any(|r| matches!(r.outcome, O::Failed { .. })) {
        t.error
    } else if e.runs.iter().any(|r| matches!(r.outcome, O::Blocked(_) | O::Timeout)) {
        t.warning
    } else {
        t.border_subtle
    };
    // Rows under the header: per run its line, then its detail lines.
    let mut rows: Vec<(Vec<Span<'static>>, Vec<Span<'static>>)> = Vec::new();
    let name_w = e.runs.iter().map(|r| r.script.chars().count()).max().unwrap_or(0);
    for r in &e.runs {
        let (mark, mark_c, what, what_c) = match &r.outcome {
            O::Ok => ("✓", t.success, "ok".to_string(), t.text_muted),
            O::Context(lines) => ("✓", t.success, format!("added {} line{} to the context", lines.len(), if lines.len() == 1 { "" } else { "s" }), t.text_muted),
            O::Blocked(_) => ("⊘", t.warning, "blocked".to_string(), t.warning),
            O::Failed { code, .. } => ("✗", t.error, format!("failed (exit {code})"), t.error),
            O::Timeout => ("◷", t.warning, "timed out".to_string(), t.warning),
            O::Running => ("◌", t.info, "running".to_string(), t.info),
        };
        rows.push((
            vec![
                Span::styled(format!("  {mark}  "), plain(mark_c)),
                Span::styled(format!("{:<name_w$}", r.script), bold(t.text)),
                Span::styled(format!("   {what}"), plain(what_c)),
            ],
            vec![Span::styled(r.duration.clone(), plain(t.text_muted))],
        ));
        let detail: Vec<(String, Color)> = match &r.outcome {
            O::Context(lines) => lines.iter().map(|l| (l.clone(), t.text_muted)).collect(),
            O::Blocked(reason) => vec![(reason.clone(), t.warning)],
            O::Failed { stderr, .. } => stderr.iter().map(|l| (l.clone(), t.error)).collect(),
            _ => vec![],
        };
        for (i, (l, c)) in detail.into_iter().enumerate() {
            let lead = if i == 0 { "↳ " } else { "  " };
            for wl in wrap(&l, iw.saturating_sub(9)) {
                rows.push((vec![Span::raw("     "), Span::styled(lead, plain(t.text_muted)), Span::styled(wl, plain(c))], vec![]));
            }
        }
    }
    let h = 3 + rows.len() as u16;
    cv.frame(Rect::new(x, y, w, h), frame, bg);
    cv.icon(ix, y + 1, 2, 1, Icon::Bolt, Tint::Info);
    let mut head = vec![Span::styled("HOOK", bold(t.info)), Span::styled(format!("   {}", e.event), bold(t.text))];
    if let Some(s) = &e.subject {
        head.push(Span::styled(format!("  {s}"), plain(t.text_muted)));
    }
    head.push(Span::styled(format!("   {} script{}", e.runs.len(), if e.runs.len() == 1 { "" } else { "s" }), plain(t.text_muted)));
    cv.line(ix + 3, y + 1, iw.saturating_sub(3), head, bg);
    cv.line_right(ix + iw, y + 1, vec![Span::styled(e.time.clone(), plain(t.text_muted))], bg);
    for (i, (left, right)) in rows.into_iter().enumerate() {
        let ry = y + 2 + i as u16;
        cv.line(ix, ry, iw, left, bg);
        if !right.is_empty() {
            cv.line_right(ix + iw, ry, right, bg);
        }
    }
    h
}

/// A plain card: stripe, a label row, body rows (wrapped), on the surface.
fn card(width: u16, t: &Theme, stripe: Color, label: Vec<Span<'static>>, body: Vec<Vec<Span<'static>>>) -> Drawn {
    let mut cv = Canvas::new(width, t);
    let tw = width.saturating_sub(CX + 2);
    let mut rows: Vec<Vec<Span<'static>>> = vec![label];
    for spans in body {
        // Wrap a body row that holds one span; rows of several spans (bullets) are kept.
        if spans.len() == 1 {
            let st = spans[0].style;
            for l in wrap(&spans[0].content, tw) {
                rows.push(vec![Span::styled(l, st)]);
            }
        } else {
            rows.push(spans);
        }
    }
    let h = rows.len() as u16;
    cv.fill(Rect::new(0, 0, width, h), t.surface);
    cv.stripe(0, h, stripe, t.surface);
    for (i, r) in rows.into_iter().enumerate() {
        cv.line(CX, i as u16, tw, r, t.surface);
    }
    cv.finish(h)
}

fn turn(width: u16, t: &Theme, role: Role, time: &str, lines: &[String], attached: &[Item], generating: bool) -> Drawn {
    let mut cv = Canvas::new(width, t);
    let tw = width.saturating_sub(CX + 2);
    let name = match role {
        Role::User => "USER",
        Role::Assistant => "ASSISTANT",
    };
    let bg = t.surface;
    cv.line(CX, 0, tw, vec![Span::styled(name, bold(t.accent)), Span::styled(format!("  {time}"), plain(t.text_muted))], bg);
    if generating {
        cv.line_right(width - 2, 0, vec![Span::styled("◌ ", plain(t.accent)), Span::styled("generating…", plain(t.text_muted))], bg);
    }
    let mut y = 1;
    for l in lines {
        for w in wrap(l, tw) {
            cv.line(CX, y, tw, vec![Span::styled(w, plain(t.text))], bg);
            y += 1;
        }
    }
    for a in attached {
        y += attached_item(&mut cv, CX, y, width.saturating_sub(CX + 1), a, t, bg);
    }
    cv.stripe(0, y, t.accent_line, bg);
    // Everything not drawn on yet takes the card's surface.
    cv.grow(y);
    for row in 0..y {
        for col in 0..width {
            if cv.buf[(col, row)].bg == Color::Reset {
                cv.buf[(col, row)].set_bg(bg);
            }
        }
    }
    cv.finish(y)
}

/// An item inside an assistant turn's card. Returns its height.
fn attached_item(cv: &mut Canvas, x: u16, y: u16, w: u16, it: &Item, t: &Theme, bg: Color) -> u16 {
    match it {
        Item::Tool(c) => tool(cv, x, y, w, c, bg),
        Item::Journal(j) => journal(cv, x, y, w, j, bg),
        Item::MapCreated(m) => map_created(cv, x, y, w, m, bg),
        Item::MindMap(e) => mind_map(cv, x, y, w, e, bg),
        Item::Flowchart(e) => flowchart(cv, x, y, w, e, bg),
        Item::Hook(e) => hook(cv, x, y, w, e, bg),
        other => {
            let d = item(other, w, t, false);
            cv.embed(x, y, d)
        }
    }
}

fn status_badge(c: &ToolCall, t: &Theme, bg: Color) -> (Vec<Span<'static>>, Color) {
    let (text, color) = match c.state {
        ToolState::Done => (format!("✓ {}", c.outcome), t.success),
        ToolState::Failed => (format!("✗ {}", c.outcome), t.error),
        ToolState::Running => ("◌ running".to_string(), t.info),
    };
    (pill_spans(&text, color, tint(color, bg, 0.16), bg), color)
}

/// A tool call: a one-line row (no body, or a diff) or a framed card (output, a sent message,
/// a note). Returns its height.
fn tool(cv: &mut Canvas, x: u16, y: u16, w: u16, c: &ToolCall, bg: Color) -> u16 {
    let t = cv.t;
    match &c.body {
        ToolBody::None | ToolBody::Diff(_) => tool_row(cv, x, y, w, c, bg),
        _ => tool_card(cv, x, y, w, c, bg, t),
    }
}

/// The compact form (main-session-*, inbound-messages-*): chevron, check, name, arguments …
/// outcome, duration; an expanded diff below.
fn tool_row(cv: &mut Canvas, x: u16, y: u16, w: u16, c: &ToolCall, bg: Color) -> u16 {
    let t = cv.t;
    let row_bg = if bg == t.surface { t.surface_raised } else { t.surface };
    cv.fill(Rect::new(x, y, w, 1), row_bg);
    let chevron = if c.expanded { "▾" } else { "▸" };
    let (mark, mark_color) = match c.state {
        ToolState::Done => ("✔", t.success),
        ToolState::Running => ("◌", t.info),
        ToolState::Failed => ("✘", t.error),
    };
    let mut left = vec![
        Span::styled(format!("  {chevron}  "), plain(t.text_muted)),
        Span::styled(mark, plain(mark_color)),
        Span::raw("  "),
        Span::styled(c.name.clone(), bold(t.text)),
    ];
    if let Some(s) = &c.args_strong {
        left.push(Span::styled(format!(" {s}"), bold(t.text)));
    }
    left.push(Span::styled(format!("  {}", c.args), plain(t.text_muted)));
    cv.line(x, y, w, left, row_bg);
    let tone = match c.outcome_tone {
        Tone::Success => t.success,
        Tone::Neutral => t.text,
        Tone::Error => t.error,
    };
    let mut right = Vec::new();
    if c.outcome_tone == Tone::Success && c.diffstat.is_none() {
        right.extend(pill_spans(&c.outcome, tone, tint(tone, row_bg, 0.16), row_bg));
    } else {
        right.push(Span::styled(c.outcome.clone(), plain(tone)));
    }
    if let Some((add, del)) = c.diffstat {
        right.push(Span::raw("   "));
        right.push(Span::styled(format!("+{add}"), plain(t.success)));
        right.push(Span::styled(" / ", plain(t.text_muted)));
        right.push(Span::styled(format!("-{del}"), plain(t.error)));
    }
    right.push(Span::styled(format!("  {:>5}  ", c.duration), plain(t.text)));
    cv.line_right(x + w, y, right, row_bg);
    let mut h = 1;
    if let (ToolBody::Diff(d), true) = (&c.body, c.expanded) {
        let (dx, dw) = (x + 3, w.saturating_sub(5));
        cv.fill(Rect::new(x, y + 1, w, d.lines.len() as u16 + 2), row_bg);
        cv.line(dx, y + 1, dw, vec![Span::styled(d.hunk.clone(), plain(t.text_muted)), Span::styled(format!("    {}", d.context), plain(t.text_muted))], row_bg);
        for (i, l) in d.lines.iter().enumerate() {
            let (sign, text, lbg, sign_fg) = match l {
                DiffLine::Removed(s) => ("-", s, t.diff_del_bg, t.error),
                DiffLine::Added(s) => ("+", s, t.diff_add_bg, t.success),
                DiffLine::Context(s) => (" ", s, row_bg, t.text_muted),
            };
            let ly = y + 2 + i as u16;
            cv.fill(Rect::new(dx, ly, dw, 1), lbg);
            cv.line(dx, ly, dw, vec![Span::styled(format!(" {sign}     "), plain(sign_fg)), Span::styled(text.clone(), plain(t.text))], lbg);
        }
        h += d.lines.len() as u16 + 2;
    }
    h
}

/// The framed card of tool-calls-*: icon, name, arguments, status badge, duration, then the
/// body. Failed calls are framed in the error colour. Body text has no background of its own:
/// only a file edit's diff is shaded (operator, 2026-11-01).
fn tool_card(cv: &mut Canvas, x: u16, y: u16, w: u16, c: &ToolCall, bg: Color, t: &Theme) -> u16 {
    let line = if c.state == ToolState::Failed { t.error } else { t.accent_line };
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let mut rows = 1u16;
    let mut body: Vec<(Vec<Span<'static>>, Option<Color>)> = Vec::new();
    match &c.body {
        ToolBody::Output { label, lines, numbered, more } => {
            let lw = label.as_ref().map_or(0, |l| l.chars().count() as u16 + 4);
            let text_color = if c.state == ToolState::Failed { t.error } else { t.text };
            for (i, l) in lines.iter().enumerate() {
                let mut spans = Vec::new();
                if *numbered {
                    spans.push(Span::styled(format!(" {:>3}  ", i + 1), plain(t.text_muted)));
                } else if let Some(lb) = label {
                    if i == 0 {
                        spans.push(Span::styled(format!("▸ {lb}: "), plain(t.text_muted)));
                    } else {
                        spans.push(Span::raw(" ".repeat(lw as usize)));
                    }
                }
                spans.push(Span::styled(l.clone(), plain(text_color)));
                if c.state == ToolState::Running && i + 1 == lines.len() {
                    spans.push(Span::styled(" █", plain(t.text)));
                }
                body.push((spans, None));
            }
            if *more > 0 {
                body.push((vec![Span::styled(format!("▸ +{more} more lines"), plain(t.text_muted))], None));
            }
        }
        // A sent message and a note: plain text under the tool's name.
        ToolBody::Bubble(text) | ToolBody::Note(text) => {
            for l in wrap(text, iw.saturating_sub(3)) {
                body.push((vec![Span::raw("   "), Span::styled(l, plain(t.text))], None));
            }
        }
        _ => {}
    }
    rows += body.len() as u16;
    let h = rows + 2;
    cv.frame(Rect::new(x, y, w, h), line, bg);
    // Header: icon, name, arguments … badge, duration.
    let icon = match c.icon {
        ToolIcon::Shell => Some(Icon::Terminal),
        ToolIcon::Send => Some(Icon::Send),
        ToolIcon::Agent => Some(Icon::Swap),
        ToolIcon::Journal => Some(Icon::File),
        ToolIcon::None => None,
    };
    let hy = y + 1;
    if let Some(i) = icon {
        cv.icon(ix, hy, 2, 1, i, Tint::Text);
    }
    cv.line(ix + 3, hy, iw.saturating_sub(3), vec![Span::styled(c.name.clone(), bold(t.accent)), Span::styled(format!("   {}", c.args), plain(t.text))], bg);
    let (mut right, _) = status_badge(c, t, bg);
    right.push(Span::styled(format!("  {:>6} ", c.duration), plain(t.text)));
    cv.line_right(ix + iw, hy, right, bg);
    for (i, (spans, row_bg)) in body.into_iter().enumerate() {
        let ry = hy + 1 + i as u16;
        match row_bg {
            Some(rb) => {
                cv.fill(Rect::new(ix, ry, iw, 1), rb);
                cv.line(ix, ry, iw, spans, rb);
            }
            None => cv.line(ix, ry, iw, spans, bg),
        }
    }
    h
}

/// The kind's colour, label, icon and meta line of an inbound card.
fn inbound_style(m: &Inbound, t: &Theme) -> (Color, &'static str, Icon, Tint) {
    match &m.kind {
        InboundKind::Telegram => (t.info, "TELEGRAM", Icon::Send, Tint::Info),
        InboundKind::Voice { .. } => (t.info, "TELEGRAM VOICE", Icon::Send, Tint::Info),
        InboundKind::Agent { .. } => (t.agent, "AGENT", Icon::Bot, Tint::Agent),
        InboundKind::Callback { .. } => (t.success, "CALLBACK", Icon::Callback, Tint::Success),
        InboundKind::Reminder { .. } => (t.agent, "REMINDER", Icon::Calendar, Tint::Agent),
    }
}

/// A message from outside (inbound-messages-*): stripe and icon in the channel's colour, the
/// channel label, sender and meta, then the text.
fn inbound(width: u16, t: &Theme, m: &Inbound) -> Drawn {
    let mut cv = Canvas::new(width, t);
    let bg = t.surface;
    let (color, label, icon, tint_) = inbound_style(m, t);
    let tx = CX + 5;
    let tw = width.saturating_sub(tx + 2);
    let mut head = vec![Span::styled(label, bold(color))];
    match &m.kind {
        InboundKind::Reminder { id, every } => {
            head.push(Span::styled(format!("  #{id}  every {every}  {}", m.time), plain(t.text_muted)));
        }
        InboundKind::Callback { transport, .. } => {
            head.push(Span::styled(format!("  {}  ({transport})  {}  ", m.from, m.time), plain(t.text_muted)));
        }
        _ => head.push(Span::styled(format!("  {}  {}  ", m.from, m.time), plain(t.text_muted))),
    }
    let mut x = tx + head.iter().map(|s| s.content.chars().count() as u16).sum::<u16>();
    let mut body: Vec<Vec<Span<'static>>> = Vec::new();
    match &m.kind {
        InboundKind::Voice { duration } => {
            // The waveform: lower block elements rising from one baseline, like a voice
            // message's bars (operator: the earlier mixed line glyphs looked crooked).
            let wave = "▃ ▅ ▇ ▄ ▆ ▃ ▅ ▇ ▄ ▂";
            head.push(Span::styled("  ▶ ", plain(color)));
            head.push(Span::styled(wave, plain(t.text_muted)));
            head.push(Span::styled(format!("  {duration}  "), plain(t.text_muted)));
            x += 4 + wave.chars().count() as u16 + duration.chars().count() as u16 + 4;
            let first = "Transcript: ".len() as u16;
            for (i, l) in wrap(&m.text, tw.saturating_sub(first)).into_iter().enumerate() {
                let lead = if i == 0 { Span::styled("Transcript: ", plain(t.text_muted)) } else { Span::raw(" ".repeat(first as usize)) };
                body.push(vec![lead, Span::styled(l, plain(t.text))]);
            }
        }
        InboundKind::Agent { session, .. } => {
            let from = format!("(from {session})  ");
            let fw = from.chars().count() as u16;
            for (i, l) in wrap(&m.text, tw.saturating_sub(fw)).into_iter().enumerate() {
                let lead = if i == 0 { Span::styled(from.clone(), plain(t.text_muted)) } else { Span::raw(" ".repeat(fw as usize)) };
                body.push(vec![lead, Span::styled(l, plain(t.text))]);
            }
        }
        _ => {
            for l in wrap(&m.text, tw) {
                body.push(vec![Span::styled(l, plain(t.text))]);
            }
        }
    }
    let h = (1 + body.len() as u16).max(2);
    cv.fill(Rect::new(0, 0, width, h), bg);
    cv.stripe(0, h, color, bg);
    cv.icon(CX, 0, 3, 2, icon, tint_);
    cv.line(tx, 0, tw, head, bg);
    // Pills after the meta: dictated, the a2a route, the callback status.
    match &m.kind {
        InboundKind::Voice { .. } => {
            cv.pill(x, 0, "dictated", t.info, tint(t.info, bg, 0.18), bg);
        }
        InboundKind::Agent { route: Some(r), .. } => {
            cv.pill(x, 0, &format!("a2a: {r}"), t.info, tint(t.info, bg, 0.14), bg);
        }
        InboundKind::Callback { status, .. } => {
            cv.pill(x, 0, status, t.success, tint(t.success, bg, 0.18), bg);
        }
        _ => {}
    }
    for (i, b) in body.into_iter().enumerate() {
        cv.line(tx, 1 + i as u16, tw, b, bg);
    }
    cv.finish(h)
}

/// A JOURNAL event: framed in the accent, `#id category`, the category and tag pills, the date,
/// then the entry's first line.
fn journal(cv: &mut Canvas, x: u16, y: u16, w: u16, j: &JournalEvent, bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let lines = wrap(&j.text, iw);
    let h = 3 + lines.len() as u16;
    cv.frame(Rect::new(x, y, w, h), t.accent_line, bg);
    cv.icon(ix, y + 1, 2, 1, Icon::File, Tint::Text);
    cv.line(ix + 3, y + 1, iw, vec![Span::styled("JOURNAL", bold(t.accent)), Span::styled(format!("   #{}  {}", j.id, j.category), plain(t.text))], bg);
    let mut right: Vec<Span<'static>> = pill_spans(&format!("✓ {}", j.category), t.success, tint(t.success, bg, 0.16), bg);
    for tag in &j.tags {
        right.push(Span::raw(" "));
        right.extend(pill_spans(tag, t.agent, tint(t.agent, bg, 0.16), bg));
    }
    right.push(Span::styled(format!("   {}", j.date), plain(t.text_muted)));
    cv.line_right(ix + iw, y + 1, right, bg);
    for (i, l) in lines.into_iter().enumerate() {
        cv.line(ix, y + 2 + i as u16, iw, vec![Span::styled(l, plain(t.text))], bg);
    }
    h
}

/// MINDMAP created: one framed line; the map has no nodes yet, so nothing is drawn.
/// The two map tools' names and what they call their maps.
fn tool_names(tool: MapTool) -> (&'static str, &'static str, Icon) {
    match tool {
        MapTool::MindMap => ("MINDMAP", "mind map", Icon::Map),
        MapTool::Flowchart => ("FLOWCHART", "flowchart", Icon::Play),
    }
}

/// A map was created: one framed line; it has no nodes yet, so nothing is drawn.
fn map_created(cv: &mut Canvas, x: u16, y: u16, w: u16, m: &MapCreated, bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let (label, noun, icon) = tool_names(m.tool);
    cv.frame(Rect::new(x, y, w, 3), t.border_subtle, bg);
    cv.icon(ix, y + 1, 2, 1, icon, Tint::Accent);
    let scope = if m.shared { "shared" } else { "private" };
    cv.line(
        ix + 3,
        y + 1,
        iw,
        vec![Span::styled(label, bold(t.accent)), Span::styled(format!("   created {noun} #{} “{}” ({scope})", m.id, m.title), plain(t.text))],
        bg,
    );
    cv.line_right(ix + iw, y + 1, vec![Span::styled("(empty)", plain(t.text_muted).add_modifier(Modifier::ITALIC))], bg);
    3
}

/// A map that changed: the event line, then the map - a mind map as a tree, a flowchart as a
/// block diagram. With pictures it is an SVG made for its cells; without, the same layout in
/// block and box-drawing characters.
fn map(cv: &mut Canvas, (x, y, w): (u16, u16, u16), tool: MapTool, (id, change, date): (u32, &str, &str), body: MapBody, bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let (label, noun, icon) = tool_names(tool);
    let map_h = match body {
        MapBody::Mind(m) => crate::mindmap::layout(m).height,
        MapBody::Flow(f) => crate::flowchart::layout(f).height,
    } + 2;
    let h = 3 + map_h;
    cv.frame(Rect::new(x, y, w, h), t.border_subtle, bg);
    cv.icon(ix, y + 1, 2, 1, icon, Tint::Accent);
    cv.line(ix + 3, y + 1, iw, vec![Span::styled(label, bold(t.accent)), Span::styled(format!("   {noun} #{id} updated: {change}"), plain(t.text))], bg);
    cv.line_right(ix + iw, y + 1, vec![Span::styled(date.to_string(), plain(t.text_muted))], bg);
    let area = Rect::new(ix, y + 2, iw, map_h);
    cv.grow(y + h);
    let svg = match body {
        MapBody::Mind(m) => {
            crate::mindmap::draw_text(&mut cv.buf, area, m, t, bg);
            crate::mindmap::svg(m, t, area.width, area.height)
        }
        MapBody::Flow(f) => {
            crate::flowchart::draw_text(&mut cv.buf, area, f, t, bg);
            crate::flowchart::svg(f, t, area.width, area.height)
        }
    };
    cv.placements.push(Placement { rect: area, graphic: Graphic::Svg(svg) });
    h
}

#[derive(Clone, Copy)]
enum MapBody<'a> {
    Mind(&'a crate::model::MindMap),
    Flow(&'a crate::model::Flowchart),
}

fn mind_map(cv: &mut Canvas, x: u16, y: u16, w: u16, e: &MindMapEvent, bg: Color) -> u16 {
    map(cv, (x, y, w), MapTool::MindMap, (e.id, &e.change, &e.date), MapBody::Mind(&e.map), bg)
}

fn flowchart(cv: &mut Canvas, x: u16, y: u16, w: u16, e: &FlowchartEvent, bg: Color) -> u16 {
    map(cv, (x, y, w), MapTool::Flowchart, (e.id, &e.change, &e.date), MapBody::Flow(&e.chart), bg)
}

/// What a spacer row needs to know about the card on one side of it.
struct Edge {
    /// The card's colour (the cell after its stripe), or None for an item on the page colour.
    surface: Option<Color>,
    /// The stripe's colour, when the card has one.
    stripe: Option<Color>,
}

fn edge(d: &Drawn, row: u16, t: &Theme) -> Edge {
    let w = d.buf.area.width;
    let bg = d.buf[(1.min(w - 1), row)].bg;
    let surface = (bg != Color::Reset && bg != t.background).then_some(bg);
    let first = &d.buf[(0, row)];
    let stripe = (first.symbol() == "▌").then_some(first.fg);
    Edge { surface, stripe }
}

/// The one row between two items (or above the first, below the last). Its top half is the
/// bottom padding of the card above, its bottom half the top padding of the card below, and a
/// thin line in the page colour runs between them: text sits off both card edges and cards are
/// a hair apart, as in the mockups. The stripes end with quadrant blocks (rounded ends).
fn spacer(above: Option<Edge>, below: Option<Edge>, width: u16, t: &Theme) -> Drawn {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
    let page = t.background;
    let (sa, sb) = (above.as_ref().and_then(|e| e.surface), below.as_ref().and_then(|e| e.surface));
    let (ta, tb) = (above.as_ref().and_then(|e| e.stripe), below.as_ref().and_then(|e| e.stripe));
    for x in 0..width {
        let cell = &mut buf[(x, 0)];
        let (left, right) = (x == 0, x + 1 == width);
        if left && (ta.is_some() || tb.is_some()) {
            // The stripe ends round: a quadrant of the card above or below, never both.
            match (ta, tb) {
                (Some(_), Some(_)) => cell.set_symbol(" ").set_bg(page),
                (Some(c), None) => cell.set_symbol("▘").set_fg(c).set_bg(page),
                (None, Some(c)) => cell.set_symbol("▖").set_fg(c).set_bg(page),
                (None, None) => unreachable!(),
            };
            continue;
        }
        // No cut corners at the card edges: in a real terminal a one-cell notch at the edge of
        // the transcript reads as a glitch, not as a rounded corner (operator's screenshot,
        // 2026-11-01). Only the stripes end round.
        let _ = (left, right);
        let (sym, fg, bg) = match (sa, sb) {
            (Some(a), Some(b)) if a == b => ("━", page, a),
            (Some(a), Some(b)) => ("▀", a, b),
            (Some(a), None) => ("▀", a, page),
            (None, Some(b)) => ("▄", b, page),
            (None, None) => (" ", page, page),
        };
        cell.set_symbol(sym).set_fg(fg).set_bg(bg);
    }
    Drawn { buf, placements: Vec::new() }
}

/// The items that reach into a view `rows` tall (the transcript's height plus the rows
/// scrolled back), drawn at `width`, oldest first, with a spacer row before each and after the
/// last; the newest assistant turn is marked busy when `generating`. The second value says
/// whether the first item was reached, i.e. the whole transcript is in the stack. Older items
/// are not drawn at all, so a frame costs what is on screen, not the length of the session.
pub fn visible(items: &[Item], width: u16, t: &Theme, generating: bool, rows: u16) -> (Vec<Drawn>, bool) {
    let last_assistant = items.iter().rposition(|i| matches!(i, Item::Turn { role: Role::Assistant, .. }));
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut acc = 0u16;
    let mut whole = true;
    for (i, it) in items.iter().enumerate().rev() {
        if acc >= rows {
            whole = false;
            break;
        }
        let d = item(it, width, t, generating && Some(i) == last_assistant);
        acc = acc.saturating_add(d.buf.area.height + 2);
        drawn.push(d);
    }
    drawn.reverse();
    let mut out = Vec::with_capacity(drawn.len() * 2 + 1);
    // Every card gets its own half-row padding above and below, which its stripe runs through,
    // so the stripe is as tall as the card; two cards are a row of page colour apart
    // (operator's choice 2026-11-01 over a shared row, where the stripes had to stop short).
    for d in drawn {
        out.push(spacer(None, Some(edge(&d, 0, t)), width, t));
        let bottom = edge(&d, d.buf.area.height - 1, t);
        out.push(d);
        out.push(spacer(Some(bottom), None, width, t));
    }
    (out, whole)
}

/// Every item drawn at `width` (tests and tools; the view draws only what is visible).
pub fn all(items: &[Item], width: u16, t: &Theme, generating: bool) -> Vec<Drawn> {
    visible(items, width, t, generating, u16::MAX).0
}

/// The transcript's total height in rows: items plus the gaps between them.
pub fn total_height(drawn: &[Drawn]) -> u16 {
    let n = drawn.len() as u16;
    drawn.iter().map(|d| d.buf.area.height).sum::<u16>() + n.saturating_sub(1) * GAP
}

/// Copies the visible part of the stack into `buf` at `area`: the bottom of the stack, raised
/// by `scroll` rows. Returns the pictures that are fully visible, in screen cells.
pub fn blit(buf: &mut Buffer, area: Rect, drawn: Vec<Drawn>, scroll: u16) -> Vec<Placement> {
    let total = total_height(&drawn);
    // Row of the stack shown at the top of `area`.
    let top = total.saturating_sub(area.height).saturating_sub(scroll);
    let mut out = Vec::new();
    let mut y0 = 0u16;
    for d in drawn {
        let h = d.buf.area.height;
        for row in 0..h {
            let sy = y0 + row;
            if sy < top || sy >= top + area.height {
                continue;
            }
            let ty = area.y + sy - top;
            for col in 0..d.buf.area.width.min(area.width) {
                buf[(area.x + col, ty)] = d.buf[(col, row)].clone();
            }
        }
        for mut p in d.placements {
            let (py, ph) = (y0 + p.rect.y, p.rect.height);
            if py >= top && py + ph <= top + area.height {
                p.rect.y = area.y + py - top;
                p.rect.x += area.x;
                out.push(p);
            }
        }
        y0 += h + GAP;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn bench_frame() {
        let t = Theme::builtin("dark").unwrap();
        let mut w = match crate::fixtures::welcome().mode { crate::model::Mode::Session(w) => w, _ => unreachable!() };
        for c in ["/agent", "/tg", "/voice", "/a2a", "/callback", "/journal", "/map", "/tool cat", "/tool fail"].iter().cycle().take(40) {
            w.view_mut().composer.text = c.to_string();
            crate::debug::submit(&mut w);
        }
        let start = std::time::Instant::now();
        for _ in 0..10 {
            let d = all(&w.view().transcript, 186, &t, false);
            std::hint::black_box(d);
        }
        eprintln!("items {} frame {:?}", w.view().transcript.len(), start.elapsed() / 10);
    }

    #[test]
    fn wrap_breaks_at_spaces_and_splits_long_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), vec!["abc", "def", "gh"]);
        assert_eq!(wrap("a\nb", 10), vec!["a", "b"]);
    }

    #[test]
    fn a_turn_grows_with_its_text_and_attachments() {
        let t = Theme::builtin("dark").unwrap();
        let short = Item::Turn { role: Role::User, time: "1".into(), lines: vec!["hi".into()], attached: vec![] };
        assert_eq!(item(&short, 40, &t, false).buf.area.height, 2);
        let long = Item::Turn { role: Role::User, time: "1".into(), lines: vec!["word ".repeat(20)], attached: vec![] };
        assert!(item(&long, 40, &t, false).buf.area.height > 3);
    }
}
