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
use crate::model::{FlowInstanceView, ThinkingView, 
    MapElement, DiffLine, FlowchartEvent, Inbound, InboundKind, Item, JournalEvent, MapCreated, MapTool, MindMapEvent, Role, Tone, ToolBody, ToolCall, ToolIcon, ToolState,
};
use crate::theme::Theme;

/// A card's stripe: a whole cell wide, so its ends can follow the quarter-row padding and gap
/// between cards (a cell holds two colours; a half-cell stripe could not end with them).
const STRIPE: &str = "█";

/// Above the agent's messages until it has a name.
pub const ASSISTANT: &str = "ASSISTANT";

/// How many lines of live reasoning its window shows.
const THINKING_LINES: usize = 5;

/// Where a card's text starts: the stripe, then two cells of air.
const CX: u16 = 3;
/// Rows between two items: none; every item ends with a half-row separator instead (see
/// `separate`), so cards sit as close as the mockups draw them.
pub const GAP: u16 = 0;
/// The tallest an item may draw.
const MAX_H: u16 = 400;

/// An item drawn on its own: its cells, its pictures and its clickable map parts (relative to
/// the buffer's origin), and which transcript item it is.
pub struct Drawn {
    pub buf: Buffer,
    pub placements: Vec<Placement>,
    /// Map parts: (cells, the attached item's index inside a turn, the part).
    pub hits: Vec<(Rect, Option<usize>, MapElement)>,
    /// The transcript index of the item (None for spacer rows).
    pub item: Option<usize>,
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
    hits: Vec<(Rect, Option<usize>, MapElement)>,
    width: u16,
    t: &'a Theme,
    /// Entries a map can unfold beside itself.
    journal: &'a [JournalEvent],
    /// The attached item being drawn inside a turn, for the hits it reports.
    sub: Option<usize>,
}

impl<'a> Canvas<'a> {
    fn new(width: u16, t: &'a Theme, journal: &'a [JournalEvent]) -> Canvas<'a> {
        Canvas { buf: Buffer::empty(Rect::new(0, 0, width, 4)), placements: Vec::new(), hits: Vec::new(), width, t, journal, sub: None }
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
            self.buf[(0, row)].set_symbol(STRIPE).set_fg(color).set_bg(bg);
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
        for (mut r, sub, e) in d.hits {
            r.x += x;
            r.y += y;
            self.hits.push((r, sub.or(self.sub), e));
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
        Drawn {
            buf,
            placements: self.placements.into_iter().filter(|p| p.rect.y + p.rect.height <= h).collect(),
            hits: self.hits.into_iter().filter(|(r, _, _)| r.y < h).collect(),
            item: None,
        }
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

/// Draws one item `width` columns wide.
pub fn item(it: &Item, width: u16, t: &Theme, known: &[JournalEvent]) -> Drawn {
    named_item(it, width, t, known, ASSISTANT)
}

/// The same, with the name above the agent's messages.
fn named_item(it: &Item, width: u16, t: &Theme, known: &[JournalEvent], agent: &str) -> Drawn {
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
        Item::Turn { role, time, lines, attached } => turn(width, t, known, *role, time, lines, attached, agent),
        Item::Tool(c) => {
            let mut cv = Canvas::new(width, t, known);
            let h = tool(&mut cv, 0, 0, width, c, t.surface);
            cv.finish(h)
        }
        Item::Inbound(m) => inbound(width, t, m),
        Item::FlowInstance(v) => {
            let mut cv = Canvas::new(width, t, known);
            let h = flow_instance(&mut cv, 0, 0, width, v, t.background);
            cv.finish(h)
        }
        Item::Thinking(v) => {
            let mut cv = Canvas::new(width, t, known);
            let h = thinking(&mut cv, 0, 0, width, v, t.background);
            cv.finish(h)
        }
        Item::Journal(j) => {
            let mut cv = Canvas::new(width, t, known);
            let h = journal(&mut cv, 0, 0, width, j, t.background);
            cv.finish(h)
        }
        Item::MapCreated(m) => {
            let mut cv = Canvas::new(width, t, known);
            let h = map_created(&mut cv, 0, 0, width, m, t.background);
            cv.finish(h)
        }
        Item::MindMap(e) => {
            let mut cv = Canvas::new(width, t, known);
            let h = mind_map(&mut cv, 0, 0, width, e, t.background);
            cv.finish(h)
        }
        Item::Flowchart(e) => {
            let mut cv = Canvas::new(width, t, known);
            let h = flowchart(&mut cv, 0, 0, width, e, t.background);
            cv.finish(h)
        }
        Item::Hook(e) => {
            let mut cv = Canvas::new(width, t, known);
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
    let mut cv = Canvas::new(width, t, &[]);
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

#[allow(clippy::too_many_arguments)]
fn turn(width: u16, t: &Theme, known: &[JournalEvent], role: Role, time: &str, lines: &[String], attached: &[Item], agent: &str) -> Drawn {
    let mut cv = Canvas::new(width, t, known);
    let tw = width.saturating_sub(CX + 2);
    let name = match role {
        Role::User => "USER",
        Role::Assistant => agent,
    };
    let bg = t.surface;
    cv.line(CX, 0, tw, vec![Span::styled(name.to_string(), bold(t.accent)), Span::styled(format!("  {time}"), plain(t.text_muted))], bg);
    let mut y = 1;
    // The reasoning comes before the answer it led to, wherever it arrived.
    for (i, a) in attached.iter().enumerate().filter(|(_, a)| matches!(a, Item::Thinking(_))) {
        cv.sub = Some(i);
        y += attached_item(&mut cv, CX, y, width.saturating_sub(CX + 1), a, t, bg);
    }
    if role == Role::User {
        // A person's message is shown as typed.
        for l in lines {
            for w in wrap(l, tw) {
                cv.line(CX, y, tw, vec![Span::styled(w, plain(t.text))], bg);
                y += 1;
            }
        }
    } else {
        y = markdown(&mut cv, lines, y, tw, t, bg);
    }
    for (i, a) in attached.iter().enumerate().filter(|(_, a)| !matches!(a, Item::Thinking(_))) {
        cv.sub = Some(i);
        y += attached_item(&mut cv, CX, y, width.saturating_sub(CX + 1), a, t, bg);
    }
    cv.sub = None;
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

/// Styled characters as spans, in runs of one style.
fn md_spans(chars: &[(char, crate::markdown::Inline)], t: &Theme, raised: Color) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut style = None;
    for &(c, s) in chars {
        if style != Some(s) {
            if let Some(done) = style.filter(|_| !run.is_empty()) {
                spans.push(Span::styled(std::mem::take(&mut run), md_style(done, t, raised)));
            }
            style = Some(s);
        }
        run.push(c);
    }
    if let Some(done) = style.filter(|_| !run.is_empty()) {
        spans.push(Span::styled(run, md_style(done, t, raised)));
    }
    spans
}

/// An answer's Markdown, from row `y`. Returns the next free row.
fn markdown(cv: &mut Canvas, lines: &[String], mut y: u16, tw: u16, t: &Theme, bg: Color) -> u16 {
    use crate::markdown::{Node, blocks, inline, wrap_styled};
    let raised = if bg == t.surface { t.surface_raised } else { t.surface };
    for node in blocks(lines) {
        match node {
            Node::Code(code) => {
                for w in wrap_keeping_indent(&code, tw.saturating_sub(2)) {
                    cv.fill(Rect::new(CX, y, tw, 1), raised);
                    cv.line(CX + 1, y, tw.saturating_sub(2), vec![Span::styled(w, plain(t.text))], raised);
                    y += 1;
                }
            }
            Node::Heading(text) => {
                let flat: String = inline(&text).into_iter().map(|(c, _)| c).collect();
                for w in wrap(&flat, tw) {
                    cv.line(CX, y, tw, vec![Span::styled(w, bold(t.accent))], bg);
                    y += 1;
                }
            }
            Node::Blank => y += 1,
            Node::Rule => {
                cv.line(CX, y, tw, vec![Span::styled("─".repeat(tw as usize), plain(t.border_subtle))], bg);
                y += 1;
            }
            Node::Text(text) => {
                for row in wrap_styled(&inline(&text), tw as usize) {
                    cv.line(CX, y, tw, md_spans(&row, t, raised), bg);
                    y += 1;
                }
            }
            Node::Quote(text) => {
                for row in wrap_styled(&inline(&text), tw.saturating_sub(2) as usize) {
                    let mut spans = vec![Span::styled("▎ ", plain(t.accent_line))];
                    spans.extend(md_spans(&row, t, raised).into_iter().map(|s| s.patch_style(plain(t.text_muted))));
                    cv.line(CX, y, tw, spans, bg);
                    y += 1;
                }
            }
            Node::Item { depth, marker, text } => {
                let x = CX + depth * 2;
                let hang = marker.chars().count() as u16 + 1;
                let room = tw.saturating_sub(depth * 2 + hang);
                for (i, row) in wrap_styled(&inline(&text), room as usize).into_iter().enumerate() {
                    if i == 0 {
                        cv.line(x, y, hang, vec![Span::styled(marker.clone(), plain(t.accent))], bg);
                    }
                    cv.line(x + hang, y, room, md_spans(&row, t, raised), bg);
                    y += 1;
                }
            }
            Node::Table { aligns, header, rows } => y = md_table(cv, &aligns, &header, &rows, y, tw, t, bg),
        }
    }
    y
}

/// A table: columns lined up by the cells they take, cells wrapping inside their column.
#[allow(clippy::too_many_arguments)]
fn md_table(cv: &mut Canvas, aligns: &[crate::markdown::Align], header: &[String], rows: &[Vec<String>], mut y: u16, tw: u16, t: &Theme, bg: Color) -> u16 {
    use crate::markdown::{Align, column_widths, display_width, inline, wrap_styled};
    let raised = if bg == t.surface { t.surface_raised } else { t.surface };
    let columns = rows.iter().map(Vec::len).chain(std::iter::once(header.len())).max().unwrap_or(0);
    if columns == 0 {
        return y;
    }
    let cell = |row: &[String], i: usize| inline(row.get(i).map_or("", String::as_str));
    let wants: Vec<usize> = (0..columns).map(|i| rows.iter().map(|r| display_width(&cell(r, i))).chain(std::iter::once(display_width(&cell(header, i)))).max().unwrap_or(0).max(1)).collect();
    let separators = 3 * (columns - 1);
    let widths = column_widths(&wants, (tw as usize).saturating_sub(separators).max(columns));
    let line = plain(t.border_subtle);
    let draw = |cv: &mut Canvas, row: &[String], head: bool, y: &mut u16| {
        let wrapped: Vec<_> = (0..columns).map(|i| wrap_styled(&cell(row, i), widths[i])).collect();
        let height = wrapped.iter().map(Vec::len).max().unwrap_or(1);
        for r in 0..height {
            let mut spans: Vec<Span<'static>> = Vec::new();
            for i in 0..columns {
                if i > 0 {
                    spans.push(Span::styled(" │ ", line));
                }
                let part: &[(char, crate::markdown::Inline)] = wrapped[i].get(r).map_or(&[], Vec::as_slice);
                let free = widths[i].saturating_sub(display_width(part));
                let (before, after) = match aligns.get(i) {
                    Some(Align::Right) => (free, 0),
                    Some(Align::Center) => (free / 2, free - free / 2),
                    _ => (0, free),
                };
                spans.push(Span::raw(" ".repeat(before)));
                if head {
                    spans.push(Span::styled(part.iter().map(|(c, _)| *c).collect::<String>(), bold(t.text)));
                } else {
                    spans.extend(md_spans(part, t, raised));
                }
                spans.push(Span::raw(" ".repeat(after)));
            }
            cv.line(CX, *y, tw, spans, bg);
            *y += 1;
        }
    };
    draw(cv, header, true, &mut y);
    let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
    cv.line(CX, y, tw, vec![Span::styled(rule.join("─┼─"), line)], bg);
    y += 1;
    for row in rows {
        draw(cv, row, false, &mut y);
    }
    y
}

fn md_style(s: crate::markdown::Inline, t: &Theme, raised: Color) -> Style {
    match s {
        crate::markdown::Inline::Plain => plain(t.text),
        crate::markdown::Inline::Bold => bold(t.text),
        crate::markdown::Inline::Code => plain(t.text).bg(raised),
        crate::markdown::Inline::Italic => plain(t.text).add_modifier(Modifier::ITALIC),
        crate::markdown::Inline::Strike => plain(t.text_muted).add_modifier(Modifier::CROSSED_OUT),
        crate::markdown::Inline::Link => plain(t.info).add_modifier(Modifier::UNDERLINED),
        crate::markdown::Inline::Muted => plain(t.text_muted),
    }
}

/// Code keeps its indentation; a long line simply continues on the next row.
fn wrap_keeping_indent(code: &str, width: u16) -> Vec<String> {
    let chars: Vec<char> = code.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(width.max(1) as usize).map(|c| c.iter().collect()).collect()
}

/// An item inside an assistant turn's card. Returns its height.
fn attached_item(cv: &mut Canvas, x: u16, y: u16, w: u16, it: &Item, t: &Theme, bg: Color) -> u16 {
    match it {
        Item::Tool(c) => tool(cv, x, y, w, c, bg),
        Item::Thinking(v) => thinking(cv, x, y, w, v, bg),
        Item::FlowInstance(v) => flow_instance(cv, x, y, w, v, bg),
        Item::Journal(j) => journal(cv, x, y, w, j, bg),
        Item::MapCreated(m) => map_created(cv, x, y, w, m, bg),
        Item::MindMap(e) => mind_map(cv, x, y, w, e, bg),
        Item::Flowchart(e) => flowchart(cv, x, y, w, e, bg),
        Item::Hook(e) => hook(cv, x, y, w, e, bg),
        other => {
            let d = item(other, w, t, cv.journal);
            cv.embed(x, y, d)
        }
    }
}

/// One instance among several (ADR 0012): a row; opened, its chart under it. An update into a
/// closed one tints the row with the accent, rising and fading by `pulse`.
fn flow_instance(cv: &mut Canvas, x: u16, y: u16, w: u16, v: &FlowInstanceView, bg: Color) -> u16 {
    let t = cv.t;
    let base = if bg == t.surface { t.surface_raised } else { t.surface };
    let row_bg = if v.pulse > 0 { tint(t.accent, base, 0.05 * v.pulse as f32) } else { base };
    cv.fill(Rect::new(x, y, w, 1), row_bg);
    let chevron = if v.expanded { "▾" } else { "▸" };
    let spans = vec![
        Span::styled(format!("{chevron} "), plain(t.text_muted)),
        Span::styled("FLOW ", bold(t.accent)),
        Span::styled(format!("#{} ", v.map), plain(t.text_muted)),
        Span::styled(v.event.change.clone(), plain(t.text)),
    ];
    cv.line(x + 1, y, w.saturating_sub(2), spans, row_bg);
    let mut h = 1;
    if v.expanded {
        let hits = cv.hits.len();
        h += flowchart(cv, x, y + 1, w, &v.event, bg);
        // No tickets or panning inside an instance yet: its row is what a click does.
        cv.hits.truncate(hits);
    }
    let sub = cv.sub;
    cv.hits.push((Rect::new(x, y, w, 1), sub, MapElement::Instance));
    h
}

/// A turn's reasoning (reasoning-*). Live: framed in the accent, with a timer and a cursor.
/// Finished: one row, `▸ Thought for 38s`; unfolded, its text under that row. Always muted: it
/// must not compete with the answer. Returns its height.
fn thinking(cv: &mut Canvas, x: u16, y: u16, w: u16, v: &ThinkingView, bg: Color) -> u16 {
    let t = cv.t;
    if v.live {
        let iw = w.saturating_sub(4);
        let mut text: Vec<String> = v.lines.iter().flat_map(|l| wrap(l, iw.saturating_sub(2))).collect();
        if text.is_empty() {
            text.push(String::new());
        }
        // A small window: the latest lines, so long reasoning does not push the answer away.
        let skip = text.len().saturating_sub(THINKING_LINES);
        text.drain(..skip);
        let h = text.len() as u16 + 3;
        cv.grow(y + h);
        cv.frame(Rect::new(x, y, w, h), t.accent_line, bg);
        cv.line(x + 2, y + 1, iw, vec![Span::styled("◌ ", plain(t.accent)), Span::styled("THINKING", bold(t.accent))], bg);
        cv.line_right(x + w - 2, y + 1, vec![Span::styled(format!("{}s", v.secs), plain(t.text_muted))], bg);
        let last = text.len() - 1;
        for (i, l) in text.into_iter().enumerate() {
            let mut spans = vec![Span::styled(l, plain(t.text_muted))];
            if i == last {
                spans.push(Span::styled(" █", plain(t.text_muted)));
            }
            cv.line(x + 2, y + 2 + i as u16, iw, spans, bg);
        }
        return h;
    }
    let row_bg = if bg == t.surface { t.surface_raised } else { t.surface };
    cv.fill(Rect::new(x, y, w, 1), row_bg);
    let chevron = if v.expanded { "▾" } else { "▸" };
    cv.line(x + 1, y, w.saturating_sub(2), vec![Span::styled(format!("{chevron} Thought for {}s", v.secs), plain(t.text_muted))], row_bg);
    let sub = cv.sub;
    cv.hits.push((Rect::new(x, y, w, 1), sub, MapElement::Thinking));
    if !v.expanded {
        return 1;
    }
    // Unfolded, it is a window of its own in the message, under its line.
    let iw = w.saturating_sub(4);
    let text: Vec<String> = v.lines.iter().flat_map(|l| wrap(l, iw)).collect();
    let h = text.len() as u16 + 2;
    cv.grow(y + 1 + h);
    cv.frame(Rect::new(x, y + 1, w, h), t.border_subtle, bg);
    for (i, l) in text.into_iter().enumerate() {
        cv.line(x + 2, y + 2 + i as u16, iw, vec![Span::styled(l, plain(t.text_muted))], bg);
    }
    1 + h
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
        InboundKind::System => (t.warning, "HARNESS", Icon::Bolt, Tint::Agent),
    }
}

/// A message from outside (inbound-messages-*): stripe and icon in the channel's colour, the
/// channel label, sender and meta, then the text.
fn inbound(width: u16, t: &Theme, m: &Inbound) -> Drawn {
    let mut cv = Canvas::new(width, t, &[]);
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

/// The journal entries an open map part unfolds: a chip's entry, or a node's or an arrow's refs.
fn opened(body: MapBody, open: MapElement) -> Vec<u32> {
    match (body, open) {
        (_, MapElement::Entry(id)) => vec![id],
        (MapBody::Mind(m), MapElement::Node(i)) => m.nodes.get(i).map(|n| n.refs.clone()).unwrap_or_default(),
        (MapBody::Flow(f), MapElement::Node(i)) => f.nodes.get(i).map(|n| n.refs.clone()).unwrap_or_default(),
        (MapBody::Flow(f), MapElement::Arrow(i)) => f.arrows.get(i).map(|a| a.refs.clone()).unwrap_or_default(),
        (MapBody::Mind(_), MapElement::Arrow(_)) | (_, MapElement::Area(..) | MapElement::Thinking | MapElement::Instance) => vec![],
    }
}

/// The width of the ticket panel beside a map.
const TICKET_W: u16 = 46;

/// The rows of the ticket panel: a header, then per entry its `#id category` line; only the
/// focused entry (else the first) shows its date, tags and text, the others stay folded. The
/// second value lists each entry's header row, for clicks.
fn ticket_rows(ids: &[u32], focus: Option<u32>, known: &[JournalEvent], w: u16, t: &Theme) -> (Vec<Vec<Span<'static>>>, Vec<(usize, u32)>) {
    let tw = w.saturating_sub(4);
    let shown = focus.filter(|f| ids.contains(f)).or(ids.first().copied());
    let mut rows: Vec<Vec<Span<'static>>> = vec![vec![Span::styled("JOURNAL", bold(t.accent)), Span::styled("   click again to fold", plain(t.text_muted))]];
    let mut heads = Vec::new();
    for id in ids {
        rows.push(vec![]);
        let open = Some(*id) == shown;
        let mark = if ids.len() > 1 { if open { "▾ " } else { "▸ " } } else { "" };
        heads.push((rows.len(), *id));
        match known.iter().find(|e| e.id == *id) {
            Some(e) => {
                rows.push(vec![Span::styled(mark, plain(t.text_muted)), Span::styled(format!("#{}", e.id), bold(t.info)), Span::styled(format!("  {}", e.category), bold(t.text))]);
                if open {
                    rows.push(vec![Span::styled(e.date.clone(), plain(t.text_muted)), Span::styled(format!("  {}", e.tags.iter().map(|g| format!("#{g}")).collect::<Vec<_>>().join(" ")), plain(t.agent))]);
                    for l in wrap(&e.text, tw) {
                        rows.push(vec![Span::styled(l, plain(t.text))]);
                    }
                }
            }
            None => rows.push(vec![Span::styled(mark, plain(t.text_muted)), Span::styled(format!("#{id}"), bold(t.info)), Span::styled("  not fetched yet", plain(t.text_muted))]),
        }
    }
    (rows, heads)
}

/// A map that changed: the event line, then the map - a mind map as a tree, a flowchart as a
/// block diagram (an SVG made for its cells with pictures, the same layout in characters
/// without). An open part unfolds its journal entries as a ticket on the right of the card.
fn map(cv: &mut Canvas, (x, y, w): (u16, u16, u16), tool: MapTool, (id, change, date): (u32, &str, &str), body: MapBody, (open, focus, pan): (Option<MapElement>, Option<u32>, (u16, u16)), bg: Color) -> u16 {
    let t = cv.t;
    let (ix, iw) = (x + 2, w.saturating_sub(4));
    let (label, noun, icon) = tool_names(tool);
    let (layout_w, layout_h) = match body {
        MapBody::Mind(m) => {
            let l = crate::mindmap::layout(m);
            (l.width, l.height)
        }
        MapBody::Flow(f) => {
            let l = crate::flowchart::layout(f);
            (l.width, l.height)
        }
    };
    // The ticket goes on the right (operator: like the mind map's); a map wider than the room
    // left is drawn from its panned position and dragged into view.
    let panel_w = TICKET_W.min(iw / 2);
    let beside = true;
    let ticket_w = panel_w;
    let ticket = open.map(|o| ticket_rows(&opened(body, o), focus, cv.journal, ticket_w, t));
    let ticket_h = ticket.as_ref().map_or(0, |(r, _)| r.len() as u16 + 2);
    let (map_h, below) = match (&ticket, beside) {
        (Some(_), true) => (layout_h.max(ticket_h) + 2, 0),
        (Some(_), false) => (layout_h + 2, ticket_h + 1),
        (None, _) => (layout_h + 2, 0),
    };
    let h = 3 + map_h + below;
    cv.frame(Rect::new(x, y, w, h), t.border_subtle, bg);
    cv.icon(ix, y + 1, 2, 1, icon, Tint::Accent);
    cv.line(ix + 3, y + 1, iw, vec![Span::styled(label, bold(t.accent)), Span::styled(format!("   {noun} #{id} updated: {change}"), plain(t.text))], bg);
    cv.line_right(ix + iw, y + 1, vec![Span::styled(date.to_string(), plain(t.text_muted))], bg);
    let side = if ticket.is_some() && beside { panel_w + 2 } else { 0 };
    let area = Rect::new(ix, y + 2, iw - side, map_h);
    cv.grow(y + h);
    let limit = crate::maps::pan_limit((area.width, area.height), (layout_w, layout_h));
    let pan = (pan.0.min(limit.0), pan.1.min(limit.1));
    let (svg, hits) = match body {
        MapBody::Mind(m) => {
            let hits = crate::mindmap::draw_text(&mut cv.buf, area, m, t, bg, pan);
            (crate::mindmap::svg(m, t, area.width, area.height, pan), hits)
        }
        MapBody::Flow(f) => {
            let hits = crate::flowchart::draw_text(&mut cv.buf, area, f, t, bg, pan);
            (crate::flowchart::svg(f, t, area.width, area.height, pan), hits)
        }
    };
    let sub = cv.sub;
    // The area first, its parts after: a press on a part wins, anywhere else starts a drag.
    cv.hits.push((area, sub, MapElement::Area(limit.0, limit.1)));
    cv.hits.extend(hits.into_iter().map(|(r, e)| (r, sub, e)));
    // Arrows where the map goes on beyond the area.
    let mid = area.y + area.height / 2;
    let mark = Style::default().fg(t.accent).bg(bg).add_modifier(Modifier::BOLD);
    if pan.0 > 0 {
        cv.buf[(area.x, mid)].set_symbol("◀").set_style(mark);
    }
    if pan.0 < limit.0 {
        cv.buf[(area.x + area.width - 1, mid)].set_symbol("▶").set_style(mark);
    }
    if limit != (0, 0) {
        cv.line_right(ix + iw, y + 1, vec![Span::styled("drag to pan   ", plain(t.text_muted)), Span::styled(date.to_string(), plain(t.text_muted))], bg);
    }
    cv.placements.push(Placement { rect: area, graphic: Graphic::Svg(svg) });
    if let Some((rows, heads)) = ticket {
        // The ticket: a frame in the accent beside the map, its rows inside.
        let pr = if beside {
            Rect::new(ix + iw - panel_w, y + 2, panel_w, (rows.len() as u16 + 2).min(map_h))
        } else {
            Rect::new(ix, y + 2 + map_h, ticket_w, rows.len() as u16 + 2)
        };
        cv.frame(pr, t.accent_line, bg);
        for (i, r) in rows.into_iter().enumerate() {
            let ry = pr.y + 1 + i as u16;
            if ry + 1 >= pr.y + pr.height {
                break;
            }
            cv.line(pr.x + 2, ry, pr.width.saturating_sub(4), r, bg);
        }
        if let Some(o) = open {
            // Clicking the ticket folds it, like clicking the part again; clicking an entry's
            // header unfolds that entry and folds the one that was open.
            cv.hits.push((pr, sub, o));
            if heads.len() > 1 {
                for (row, entry) in heads {
                    cv.hits.push((Rect::new(pr.x + 1, pr.y + 1 + row as u16, pr.width.saturating_sub(2), 1), sub, MapElement::Entry(entry)));
                }
            }
        }
    }
    h
}

#[derive(Clone, Copy)]
enum MapBody<'a> {
    Mind(&'a crate::model::MindMap),
    Flow(&'a crate::model::Flowchart),
}

fn mind_map(cv: &mut Canvas, x: u16, y: u16, w: u16, e: &MindMapEvent, bg: Color) -> u16 {
    map(cv, (x, y, w), MapTool::MindMap, (e.id, &e.change, &e.date), MapBody::Mind(&e.map), (e.open, e.focus, e.pan), bg)
}

fn flowchart(cv: &mut Canvas, x: u16, y: u16, w: u16, e: &FlowchartEvent, bg: Color) -> u16 {
    map(cv, (x, y, w), MapTool::Flowchart, (e.id, &e.change, &e.date), MapBody::Flow(&e.chart), (e.open, e.focus, e.pan), bg)
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
    let stripe = (first.symbol() == STRIPE).then_some(first.fg);
    Edge { surface, stripe }
}

/// The one row between two items, and above the first and below the last: half a row of padding
/// for the card on each side, and between two cards a hair of the page colour, so text sits off
/// the edges and the cards stay close (the owner's choice, 2026-11-01).
fn spacer(above: Option<Edge>, below: Option<Edge>, width: u16, t: &Theme) -> Drawn {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, 1));
    let page = t.background;
    let (sa, sb) = (above.as_ref().and_then(|e| e.surface), below.as_ref().and_then(|e| e.surface));
    let (ta, tb) = (above.as_ref().and_then(|e| e.stripe), below.as_ref().and_then(|e| e.stripe));
    for x in 0..width {
        let cell = &mut buf[(x, 0)];
        let striped = x == 0 && (ta.is_some() || tb.is_some());
        let (above_c, below_c) = if striped { (ta, tb) } else { (sa, sb) };
        let (sym, fg, bg) = match (above_c, below_c) {
            (Some(a), Some(b)) if a == b => ("━", page, a),
            (Some(a), Some(b)) => ("▄", b, a),
            (Some(a), None) => ("▀", a, page),
            (None, Some(b)) => ("▄", b, page),
            (None, None) => (" ", page, page),
        };
        cell.set_symbol(sym).set_fg(fg).set_bg(bg);
    }
    Drawn { buf, placements: Vec::new(), hits: Vec::new(), item: None }
}

/// The items that reach into a view `rows` tall (the transcript's height plus the rows
/// scrolled back), drawn at `width`, oldest first, with a spacer row before each and after the
/// last. The second value says
/// whether the first item was reached, i.e. the whole transcript is in the stack. Older items
/// are not drawn at all, so a frame costs what is on screen, not the length of the session.
pub fn visible(items: &[Item], width: u16, t: &Theme, known: &[JournalEvent], agent: &str, rows: u16) -> (Vec<Drawn>, bool) {
    let mut drawn: Vec<Drawn> = Vec::new();
    let mut acc = 0u16;
    let mut whole = true;
    for (i, it) in items.iter().enumerate().rev() {
        if acc >= rows {
            whole = false;
            break;
        }
        let mut d = named_item(it, width, t, known, agent);
        d.item = Some(i);
        acc = acc.saturating_add(d.buf.area.height + 1);
        drawn.push(d);
    }
    drawn.reverse();
    let mut out = Vec::with_capacity(drawn.len() * 2 + 1);
    let mut above = None;
    for d in drawn {
        out.push(spacer(above.take(), Some(edge(&d, 0, t)), width, t));
        above = Some(edge(&d, d.buf.area.height - 1, t));
        out.push(d);
    }
    if above.is_some() {
        out.push(spacer(above, None, width, t));
    }
    (out, whole)
}

/// Every item drawn at `width` (tests and tools; the view draws only what is visible).
pub fn all(items: &[Item], width: u16, t: &Theme) -> Vec<Drawn> {
    visible(items, width, t, &[], ASSISTANT, u16::MAX).0
}

/// The transcript's total height in rows: items plus the gaps between them.
pub fn total_height(drawn: &[Drawn]) -> u16 {
    let n = drawn.len() as u16;
    drawn.iter().map(|d| d.buf.area.height).sum::<u16>() + n.saturating_sub(1) * GAP
}

/// Copies the visible part of the stack into `buf` at `area`: the bottom of the stack, raised
/// by `scroll` rows. Returns the pictures that are fully visible, in screen cells.
/// A map part on screen: its cells, the transcript item, the attached item inside it, the part.
pub type MapHit = (Rect, usize, Option<usize>, MapElement);

pub fn blit(buf: &mut Buffer, area: Rect, drawn: Vec<Drawn>, scroll: u16) -> (Vec<Placement>, Vec<MapHit>) {
    let total = total_height(&drawn);
    // Row of the stack shown at the top of `area`.
    let top = total.saturating_sub(area.height).saturating_sub(scroll);
    let mut out = Vec::new();
    let mut hits = Vec::new();
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
        // Clickable map parts: the rows of them that are in view.
        if let Some(item) = d.item {
            for (r, sub, e) in d.hits {
                let ry = y0 + r.y;
                let (from, to) = (ry.max(top), (ry + r.height).min(top + area.height));
                if from < to {
                    hits.push((Rect::new(area.x + r.x, area.y + from - top, r.width, to - from), item, sub, e));
                }
            }
        }
        y0 += h + GAP;
    }
    (out, hits)
}

#[cfg(test)]
mod markdown_tests {
    use super::*;

    fn rows(lines: &[&str], width: u16) -> Vec<String> {
        let t = Theme::builtin("dark").unwrap();
        let it = Item::Turn { role: Role::Assistant, time: "1".into(), lines: lines.iter().map(|l| l.to_string()).collect(), attached: vec![] };
        let d = item(&it, width, &t, &[]);
        (0..d.buf.area.height).map(|y| {
            let mut skip = 0;
            let mut out = String::new();
            for x in 0..d.buf.area.width {
                let s = d.buf[(x, y)].symbol();
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                skip = s.chars().map(crate::markdown::cell_width).sum::<usize>().saturating_sub(1);
                out.push_str(s);
            }
            out
        }).collect()
    }

    /// The answer of the owner's screenshot, 2026-11-01.
    #[test]
    fn the_owners_table_lines_up_with_its_emoji() {
        let shown = rows(&[
            "Проверил **19 из 23**:",
            "",
            "| Группа | Инструменты | Результат |",
            "|---|---|---|",
            "| Файлы и команды | `read`, `edit` | ✅ Работают |",
            "| HTTP | `fetch` | ✅ Получен HTML |",
            "",
            "**Особенности:**",
            "- `journal_close` сначала отказался",
            "- `search` с путём",
        ], 90);
        let all = shown.join("\n");
        assert!(!all.contains("|---") && !all.contains("| Группа") && !all.contains("**") && !all.contains('`') && !all.contains("- `"), "{all}");
        let col = |needle: &str| {
            let row = shown.iter().find(|r| r.contains(needle)).unwrap();
            // Display columns of the separators in this row.
            let mut at = 0;
            let mut seps = Vec::new();
            for c in row.chars() {
                if c == '│' {
                    seps.push(at);
                }
                at += crate::markdown::cell_width(c);
            }
            seps
        };
        assert_eq!(col("Группа").len(), 2, "{all}");
        assert_eq!(col("Группа"), col("Файлы"), "{all}");
        assert_eq!(col("Группа"), col("HTTP"), "{all}");
        assert!(all.contains("┼"), "{all}");
        assert!(all.contains("• journal_close") && all.contains("• search"), "{all}");
    }

    #[test]
    fn a_wide_table_wraps_inside_its_cells_and_a_list_item_hangs() {
        let shown = rows(&["| Name | Text |", "|---|---|", "| a | one two three four five six seven eight nine ten eleven twelve |"], 40);
        let all = shown.join("\n");
        assert!(all.contains("twelve") && all.contains("one two"), "{all}");
        let body: Vec<&String> = shown.iter().filter(|r| r.contains('│')).collect();
        assert!(body.len() >= 4, "header and wrapped rows: {all}");
        let first = body[0].find('│').unwrap();
        assert!(body.iter().all(|r| r.find('│') == Some(first)), "{all}");
        let list = rows(&["- one two three four five six seven eight nine ten eleven"], 30);
        let letter = |r: &str| r.chars().position(char::is_alphabetic).unwrap();
        assert_eq!(letter(&list[1]), letter(&list[2]), "the wrap hangs under the text: {list:?}");
    }
}

#[cfg(test)]
mod thinking_tests {
    use super::*;

    fn rows(d: &Drawn) -> Vec<String> {
        (0..d.buf.area.height).map(|y| (0..d.buf.area.width).map(|x| d.buf[(x, y)].symbol().to_string()).collect::<String>()).collect()
    }

    fn turn_with(view: ThinkingView) -> Item {
        Item::Turn { role: Role::Assistant, time: "12:00".into(), lines: vec!["The answer.".into()], attached: vec![Item::Thinking(view)] }
    }

    #[test]
    fn reasoning_has_a_small_window_in_the_message() {
        let t = Theme::builtin("dark").unwrap();
        let lines: Vec<String> = (0..20).map(|i| format!("thought {i}")).collect();
        let live = rows(&item(&turn_with(ThinkingView { lines: lines.clone(), live: true, secs: 3, expanded: false }), 60, &t, &[]));
        let all = live.join("\n");
        assert!(all.contains("THINKING") && all.contains("thought 19") && !all.contains("thought 14 "), "{all}");
        assert!(all.contains("thought 15") && all.contains("The answer."), "{all}");
        assert!(live.iter().any(|r| r.contains('╭')), "framed: {all}");

        let folded = rows(&item(&turn_with(ThinkingView { lines: lines.clone(), live: false, secs: 3, expanded: false }), 60, &t, &[])).join("\n");
        assert!(folded.contains("Thought for 3s") && !folded.contains("thought 1"), "{folded}");

        let open = rows(&item(&turn_with(ThinkingView { lines, live: false, secs: 3, expanded: true }), 60, &t, &[]));
        let text = open.join("\n");
        assert!(text.contains("thought 0") && text.contains("thought 19") && open.iter().any(|r| r.contains('╭')), "{text}");
    }

    #[test]
    fn the_agents_name_replaces_the_word_and_a_persons_card_keeps_its_own() {
        let t = Theme::builtin("dark").unwrap();
        let text = |it: &Item| rows(&named_item(it, 60, &t, &[], "Vega")).join("\n");
        let answer = turn_with(ThinkingView::default());
        assert!(text(&answer).contains("Vega") && !text(&answer).contains("ASSISTANT"));
        let person = Item::Turn { role: Role::User, time: "1".into(), lines: vec!["hi".into()], attached: vec![] };
        assert!(text(&person).contains("USER") && !text(&person).contains("Vega"));
        assert!(rows(&item(&answer, 60, &t, &[])).join("\n").contains("ASSISTANT"), "until it has a name");
    }

    #[test]
    fn no_card_says_generating() {
        let t = Theme::builtin("dark").unwrap();
        let text = rows(&item(&turn_with(ThinkingView::default()), 60, &t, &[])).join("\n");
        assert!(!text.contains("generating"), "{text}");
    }
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
            let d = all(&w.view().transcript, 186, &t);
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
        assert_eq!(item(&short, 40, &t, &[]).buf.area.height, 2);
        let long = Item::Turn { role: Role::User, time: "1".into(), lines: vec!["word ".repeat(20)], attached: vec![] };
        assert!(item(&long, 40, &t, &[]).buf.area.height > 3);
    }

    /// The marks do not reach the screen, a bold run keeps its style across a wrap, and the text
    /// is complete after wrapping.
    #[test]
    fn an_answer_draws_as_markdown_and_a_person_as_typed() {
        use ratatui::style::Modifier;
        let t = Theme::builtin("dark").unwrap();
        let row = |d: &Drawn, y: u16, w: u16| -> String { (0..w).map(|x| d.buf[(x, y)].symbol().chars().next().unwrap_or(' ')).collect() };
        let answer = "The answer is **a bold stretch long enough to wrap around the narrow card here** and `code`.";
        let turn = |role| Item::Turn { role, time: "1".into(), lines: vec![answer.into(), "```".into(), "    x = 1".into(), "```".into()], attached: vec![] };
        let d = item(&turn(Role::Assistant), 40, &t, &[]);
        let all: String = (0..d.buf.area.height).map(|y| row(&d, y, 40)).collect::<Vec<_>>().join("|");
        assert!(!all.contains("**") && !all.contains('`'), "{all}");
        for word in ["answer", "bold", "stretch", "narrow", "wrap", "code", "x = 1"] {
            assert!(all.contains(word), "{word} in {all}");
        }
        assert!(!all.contains("``"), "{all}");
        let bold_at = |word: &str| -> bool {
            for y in 0..d.buf.area.height {
                let r = row(&d, y, 40);
                if let Some(x) = r.find(word) {
                    let x = r[..x].chars().count() as u16;
                    return d.buf[(x, y)].modifier.contains(Modifier::BOLD);
                }
            }
            panic!("{word} not drawn");
        };
        assert!(bold_at("stretch") && bold_at("narrow"), "bold on both sides of the wrap");
        assert!(!bold_at("answer"));

        let d = item(&turn(Role::User), 40, &t, &[]);
        let all: String = (0..d.buf.area.height).map(|y| row(&d, y, 40)).collect::<Vec<_>>().join("|");
        assert!(all.contains("**") && all.contains("```"), "as typed: {all}");
    }
}
