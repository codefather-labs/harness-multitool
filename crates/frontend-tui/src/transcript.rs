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

use crate::flowmap;
use crate::images::{Graphic, Icon, Placement, Tint};
use crate::model::{
    DiffLine, Inbound, InboundKind, Item, JournalEvent, MapCreated, MapEvent, Role, Tone, ToolBody, ToolCall, ToolIcon, ToolState,
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
        Item::Map(m) => {
            let mut cv = Canvas::new(width, t);
            let h = map(&mut cv, 0, 0, width, m, t.background);
            cv.finish(h)
        }
    }
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
        Item::Map(m) => map(cv, x, y, w, m, bg),
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
/// body. Failed calls are framed in the error colour.
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
                body.push((spans, if *numbered { Some(t.background) } else { None }));
            }
            if *more > 0 {
                body.push((vec![Span::styled(format!("▸ +{more} more lines"), plain(t.text_muted))], None));
            }
        }
        ToolBody::Bubble(text) => {
            for l in wrap(text, iw.saturating_sub(4)) {
                body.push((vec![Span::raw(l)], Some(Color::Reset)));
            }
        }
        ToolBody::Note(text) => {
            for l in wrap(text, iw.saturating_sub(2)) {
                body.push((vec![Span::styled(format!(" {l}"), plain(t.text))], Some(t.background)));
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
            // A bubble: the sent text in an info-tinted pill.
            Some(Color::Reset) => {
                let text: String = spans.iter().map(|s| s.content.to_string()).collect();
                let fill = tint(t.info, bg, 0.14);
                let mut spans = pill_spans(&format!(" {text} "), t.text, fill, bg);
                spans[1].style = spans[1].style.remove_modifier(Modifier::BOLD);
                let w = text.chars().count() as u16 + 4;
                cv.line(ix + 2, ry, w, spans, bg);
            }
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
            head.push(Span::styled("  ▶ ", plain(t.text)));
            // The waveform as separate thin bars, like the mockup's.
            let wave = "╻╹┃╻│┃╹┃│╻┃╹";
            head.push(Span::styled(wave, plain(t.text)));
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
fn map_created(cv: &mut Canvas, x: u16, y: u16, w: u16, m: &MapCreated, bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    cv.frame(Rect::new(x, y, w, 3), t.border_subtle, bg);
    cv.icon(ix, y + 1, 2, 1, Icon::Map, Tint::Accent);
    cv.line(
        ix + 3,
        y + 1,
        iw,
        vec![Span::styled("MINDMAP", bold(t.accent)), Span::styled(format!("   created map #{} “{}” ({})", m.id, m.title, m.kind), plain(t.text))],
        bg,
    );
    cv.line_right(ix + iw, y + 1, vec![Span::styled("(empty)", plain(t.text_muted).add_modifier(Modifier::ITALIC))], bg);
    3
}

/// MINDMAP updated: the event line, then the map. With pictures the map is an SVG made for its
/// cells; without, the same layout in box-drawing characters.
fn map(cv: &mut Canvas, x: u16, y: u16, w: u16, m: &MapEvent, bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let l = flowmap::layout(&m.map);
    let map_h = l.height + 2;
    let h = 3 + map_h;
    cv.frame(Rect::new(x, y, w, h), t.border_subtle, bg);
    cv.icon(ix, y + 1, 2, 1, Icon::Map, Tint::Accent);
    cv.line(ix + 3, y + 1, iw, vec![Span::styled("MINDMAP", bold(t.accent)), Span::styled(format!("   map #{} updated: {}", m.id, m.change), plain(t.text))], bg);
    cv.line_right(ix + iw, y + 1, vec![Span::styled(m.date.clone(), plain(t.text_muted))], bg);
    let area = Rect::new(ix, y + 2, iw, map_h);
    cv.grow(y + h);
    flowmap::draw_text(&mut cv.buf, area, &m.map, t, bg);
    cv.placements.push(Placement { rect: area, graphic: Graphic::Svg(flowmap::svg(&m.map, t, area.width, area.height)) });
    h
}

/// Pads an item with half a row above and below: the first and last rows are half blocks
/// whose inner half takes the colour of the card's edge row (its surface, its stripe) and
/// whose outer half is the page. The text then sits in the middle of its card with equal
/// air above and below, and two cards are a row apart, as in the mockups.
fn separate(d: Drawn, t: &Theme) -> Drawn {
    let (w, h) = (d.buf.area.width, d.buf.area.height);
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h + 2));
    for y in 0..h {
        for x in 0..w {
            buf[(x, y + 1)] = d.buf[(x, y)].clone();
        }
    }
    // The card's own colour: the cell after the stripe on its first row (a label row, never a
    // pill or a nested row). Contents of the edge rows (pills, nested tool rows) do not leak
    // into the padding.
    let card = match d.buf[(1.min(w - 1), 0)].bg {
        Color::Reset => t.background,
        c => c,
    };
    let stripe = d.buf[(0, 0)].clone();
    let striped = stripe.symbol() == "▌";
    for (row, top) in [(0, true), (h + 1, false)] {
        for x in 0..w {
            let cell = &mut buf[(x, row)];
            if x == 0 && striped {
                cell.set_symbol(if top { "▖" } else { "▘" }).set_fg(stripe.fg).set_bg(t.background);
            } else {
                cell.set_symbol(if top { "▄" } else { "▀" }).set_fg(card).set_bg(t.background);
            }
        }
    }
    let placements = d
        .placements
        .into_iter()
        .map(|mut p| {
            p.rect.y += 1;
            p
        })
        .collect();
    Drawn { buf, placements }
}

/// The items that reach into a view `rows` tall (the transcript's height plus the rows
/// scrolled back), drawn at `width`, oldest first; the newest assistant turn is marked busy
/// when `generating`. The second value says whether the first item was reached, i.e. the
/// whole transcript is in the stack. Older items are not drawn at all, so a frame costs what
/// is on screen, not the length of the session.
pub fn visible(items: &[Item], width: u16, t: &Theme, generating: bool, rows: u16) -> (Vec<Drawn>, bool) {
    let last_assistant = items.iter().rposition(|i| matches!(i, Item::Turn { role: Role::Assistant, .. }));
    let mut out = Vec::new();
    let mut acc = 0u16;
    for (i, it) in items.iter().enumerate().rev() {
        if acc >= rows {
            out.reverse();
            return (out, false);
        }
        let d = separate(item(it, width, t, generating && Some(i) == last_assistant), t);
        acc = acc.saturating_add(d.buf.area.height + GAP);
        out.push(d);
    }
    out.reverse();
    (out, true)
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
