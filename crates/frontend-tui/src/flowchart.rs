//! Flowcharts (ADR 0011: how something flows from one point to another): ISO 5807 symbols and
//! logic gates in ranks by their distance from the start, left to right or top to bottom,
//! joined by arrows that may carry a condition. Each symbol has its own shape in text (a
//! filled card with its own ends) and in SVG.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::maps::{CH, CW, Put, esc, hex, mix, svg_open, wrap};
use crate::model::{FlowDirection, Flowchart, Gate, Symbol};
use crate::theme::Theme;

/// Between two ranks: columns when the flow runs right, rows when it runs down.
const RANK_GAP_RIGHT: u16 = 8;
const RANK_GAP_DOWN: u16 = 4;
/// Between nodes of one rank: rows when the flow runs right, columns when it runs down.
const NODE_GAP_RIGHT: u16 = 1;
const NODE_GAP_DOWN: u16 = 4;
/// Widest text line in a node.
const TEXT_MAX: usize = 16;

#[derive(Debug, Clone)]
struct Placed {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    lines: Vec<String>,
    symbol: Symbol,
    changed: bool,
}

#[derive(Debug, Clone)]
pub struct Layout {
    direction: FlowDirection,
    nodes: Vec<Placed>,
    /// (from, to, condition) by node index.
    arrows: Vec<(usize, usize, Option<String>)>,
    pub width: u16,
    pub height: u16,
}

/// A gate's name and its IEC 60617 symbol; ` o` marks an inverted output.
pub fn gate_symbol(g: Gate) -> (&'static str, &'static str) {
    match g {
        Gate::And => ("AND", "&"),
        Gate::Or => ("OR", "≥1"),
        Gate::Not => ("NOT", "1 o"),
        Gate::Nand => ("NAND", "& o"),
        Gate::Nor => ("NOR", "≥1 o"),
        Gate::Xor => ("XOR", "=1"),
        Gate::Xnor => ("XNOR", "=1 o"),
    }
}

pub fn layout(chart: &Flowchart) -> Layout {
    let n = chart.nodes.len();
    let index = |id: &str| chart.nodes.iter().position(|x| x.id == id);
    let arrows: Vec<(usize, usize, Option<String>)> =
        chart.arrows.iter().filter_map(|a| Some((index(&a.from)?, index(&a.to)?, a.label.clone()))).collect();
    // Rank: longest path from a node without incoming arrows (loops cut by the iteration cap).
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        for &(a, b, _) in &arrows {
            if rank[b] < rank[a] + 1 && rank[a] + 1 < n {
                rank[b] = rank[a] + 1;
            }
        }
    }
    let ranks = rank.iter().copied().max().map_or(0, |r| r + 1);
    let mut nodes: Vec<Placed> = chart
        .nodes
        .iter()
        .map(|nd| {
            let lines = match nd.symbol {
                Symbol::Connector => vec![nd.text.clone()],
                Symbol::Gate(g) => {
                    let (name, sym) = gate_symbol(g);
                    vec![name.to_string(), sym.to_string()]
                }
                _ => {
                    let mut l = vec![nd.id.clone()];
                    l.extend(wrap(&nd.text, TEXT_MAX));
                    l
                }
            };
            let text_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as u16;
            let extra = match nd.symbol {
                Symbol::Decision => 8,
                Symbol::Start | Symbol::End | Symbol::Prep | Symbol::Io | Symbol::Manual | Symbol::Subprocess => 6,
                Symbol::Delay | Symbol::Connector => 5,
                _ => 4,
            };
            let h = lines.len() as u16 + 2 + u16::from(nd.symbol == Symbol::Data);
            Placed { x: 0, y: 0, w: text_w + extra, h, lines, symbol: nd.symbol, changed: nd.changed }
        })
        .collect();
    let members: Vec<Vec<usize>> = (0..ranks).map(|r| (0..n).filter(|&i| rank[i] == r).collect()).collect();
    let (width, height) = match chart.direction {
        FlowDirection::Right => {
            // Ranks are columns; a rank's nodes stack, centred on the tallest column.
            let mut x = 0u16;
            let mut cols = Vec::new();
            for m in &members {
                let w = m.iter().map(|&i| nodes[i].w).max().unwrap_or(0);
                let h: u16 = m.iter().map(|&i| nodes[i].h).sum::<u16>() + NODE_GAP_RIGHT * m.len().saturating_sub(1) as u16;
                cols.push((x, w, h));
                x += w + RANK_GAP_RIGHT;
            }
            let height = cols.iter().map(|c| c.2).max().unwrap_or(0);
            for (m, &(cx, cw, ch)) in members.iter().zip(&cols) {
                let mut y = (height - ch) / 2;
                for &i in m {
                    nodes[i].x = cx + (cw - nodes[i].w) / 2;
                    nodes[i].y = y;
                    y += nodes[i].h + NODE_GAP_RIGHT;
                }
            }
            (x.saturating_sub(RANK_GAP_RIGHT), height)
        }
        FlowDirection::Down => {
            // Ranks are rows; a rank's nodes sit side by side, centred on the widest row.
            let mut y = 0u16;
            let mut rows = Vec::new();
            for m in &members {
                let h = m.iter().map(|&i| nodes[i].h).max().unwrap_or(0);
                let w: u16 = m.iter().map(|&i| nodes[i].w).sum::<u16>() + NODE_GAP_DOWN * m.len().saturating_sub(1) as u16;
                rows.push((y, h, w));
                y += h + RANK_GAP_DOWN;
            }
            let width = rows.iter().map(|r| r.2).max().unwrap_or(0);
            for (m, &(ry, rh, rw)) in members.iter().zip(&rows) {
                let mut x = (width - rw) / 2;
                for &i in m {
                    nodes[i].x = x;
                    nodes[i].y = ry + (rh - nodes[i].h) / 2;
                    x += nodes[i].w + NODE_GAP_DOWN;
                }
            }
            (width, y.saturating_sub(RANK_GAP_DOWN))
        }
    };
    Layout { direction: chart.direction, nodes, arrows, width, height }
}

/// An arrow as cell points: (start, bend, end). Right: out of a's right middle, across to the
/// middle of the gap, up or down, into b's left middle. Down: out of a's bottom middle, down to
/// the middle of the gap, across, down into b's top middle.
fn route(l: &Layout, a: usize, b: usize) -> ((u16, u16), u16, (u16, u16)) {
    let (pa, pb) = (&l.nodes[a], &l.nodes[b]);
    match l.direction {
        FlowDirection::Right => {
            let (x0, y0) = (pa.x + pa.w, pa.y + pa.h / 2);
            let (x1, y1) = (pb.x, pb.y + pb.h / 2);
            ((x0, y0), x0 + x1.saturating_sub(x0) / 2, (x1, y1))
        }
        FlowDirection::Down => {
            let (x0, y0) = (pa.x + pa.w / 2, pa.y + pa.h);
            let (x1, y1) = (pb.x + pb.w / 2, pb.y);
            ((x0, y0), y0 + y1.saturating_sub(y0) / 2, (x1, y1))
        }
    }
}

/// A symbol's fill and line colour.
fn colours(p: &Placed, t: &Theme, bg: Color) -> (Color, Color) {
    if p.changed {
        return (mix(t.accent_line, bg, 0.30), t.accent_line);
    }
    match p.symbol {
        Symbol::Start => (mix(t.success, bg, 0.16), t.success),
        Symbol::End => (mix(t.error, bg, 0.14), t.error),
        Symbol::Decision => (mix(t.accent_line, bg, 0.14), t.accent_line),
        Symbol::Gate(_) => (mix(t.info, bg, 0.14), t.info),
        Symbol::Io | Symbol::Manual => (mix(t.info, bg, 0.10), t.info),
        Symbol::Data | Symbol::Document => (mix(t.agent, bg, 0.12), t.agent),
        Symbol::Note | Symbol::Connector => (mix(t.text_muted, bg, 0.12), t.text_muted),
        _ => (t.surface_raised, t.border_subtle),
    }
}

/// The chart in text, centred in `area`.
pub fn draw_text(buf: &mut Buffer, area: Rect, chart: &Flowchart, t: &Theme, bg: Color) {
    let l = layout(chart);
    let ox = area.x + area.width.saturating_sub(l.width) / 2;
    let oy = area.y + area.height.saturating_sub(l.height) / 2;
    let mut put = |buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style| {
        let pos = (ox + x, oy + y);
        if area.contains(pos.into()) {
            buf[pos].set_symbol(s).set_style(st);
        }
    };
    let edge = Style::default().fg(t.text_muted).bg(bg);
    for (a, b, label) in &l.arrows {
        let ((x0, y0), bend, (x1, y1)) = route(&l, *a, *b);
        match l.direction {
            FlowDirection::Right => {
                for x in x0..bend {
                    put(buf, x, y0, "─", edge);
                }
                if y0 != y1 {
                    for y in y0.min(y1) + 1..y0.max(y1) {
                        put(buf, bend, y, "│", edge);
                    }
                    put(buf, bend, y0, if y1 > y0 { "╮" } else { "╯" }, edge);
                    put(buf, bend, y1, if y1 > y0 { "╰" } else { "╭" }, edge);
                } else {
                    put(buf, bend, y0, "─", edge);
                }
                for x in bend + 1..x1.saturating_sub(1) {
                    put(buf, x, y1, "─", edge);
                }
                put(buf, x1.saturating_sub(1), y1, "▶", edge);
                if let Some(lb) = label {
                    pill(buf, bend + 1, y1, lb, t, bg, &mut put);
                }
            }
            FlowDirection::Down => {
                for y in y0..bend {
                    put(buf, x0, y, "│", edge);
                }
                if x0 != x1 {
                    for x in x0.min(x1) + 1..x0.max(x1) {
                        put(buf, x, bend, "─", edge);
                    }
                    put(buf, x0, bend, if x1 > x0 { "╰" } else { "╯" }, edge);
                    put(buf, x1, bend, if x1 > x0 { "╮" } else { "╭" }, edge);
                } else {
                    put(buf, x0, bend, "│", edge);
                }
                for y in bend + 1..y1.saturating_sub(1) {
                    put(buf, x1, y, "│", edge);
                }
                put(buf, x1, y1.saturating_sub(1), "▼", edge);
                if let Some(lb) = label {
                    pill(buf, x1 + 1, bend + 1, lb, t, bg, &mut put);
                }
            }
        }
    }
    for p in &l.nodes {
        node(buf, p, t, bg, &mut put);
    }
}

/// A condition label as a pill sitting on the arrow.
fn pill(buf: &mut Buffer, x: u16, y: u16, text: &str, t: &Theme, bg: Color, put: &mut Put) {
    let fill = t.surface_raised;
    put(buf, x, y, "▐", Style::default().fg(fill).bg(bg));
    for (i, ch) in text.chars().enumerate() {
        put(buf, x + 1 + i as u16, y, &ch.to_string(), Style::default().fg(t.text).bg(fill));
    }
    put(buf, x + 1 + text.chars().count() as u16, y, "▌", Style::default().fg(fill).bg(bg));
}

/// One symbol as a filled card. Edges are half blocks in the fill colour so the fill starts at
/// the edge; each symbol gets its own ends (round, pointed, slanted, folded).
fn node(buf: &mut Buffer, p: &Placed, t: &Theme, bg: Color, put: &mut Put) {
    let (fill, line) = colours(p, t, bg);
    let (r, b) = (p.x + p.w - 1, p.y + p.h - 1);
    let mids = p.h.saturating_sub(2);
    // Whether row y lies in the upper half of the symbol's inner rows.
    let upper = |y: u16| (y - p.y - 1) * 2 < mids;
    let on = |s: &'static str| (s, Style::default().fg(fill).bg(bg));
    let none = ("", Style::default());
    let symbol = p.symbol;
    for y in p.y..=b {
        for x in p.x..=r {
            let (left, right, top, bottom) = (x == p.x, x == r, y == p.y, y == b);
            let corner = (left || right) && (top || bottom);
            let (sym, st): (&str, Style) = if !(left || right || top || bottom) {
                (" ", Style::default().bg(fill))
            } else {
                match symbol {
                    // Rounded ends: notched quadrant caps.
                    Symbol::Start | Symbol::End | Symbol::Connector => {
                        if corner {
                            none
                        } else if left {
                            on(if mids <= 1 { "▐" } else if upper(y) { "▟" } else { "▜" })
                        } else if right {
                            on(if mids <= 1 { "▌" } else if upper(y) { "▙" } else { "▛" })
                        } else if top {
                            on(if x == p.x + 1 || x == r - 1 { "▗" } else { "▄" })
                        } else {
                            on(if x == p.x + 1 || x == r - 1 { "▝" } else { "▀" })
                        }
                    }
                    // Pointed ends; a decision also tapers top and bottom (a diamond),
                    // preparation keeps them flat (a hexagon).
                    Symbol::Decision | Symbol::Prep => {
                        let taper = symbol == Symbol::Decision && (x <= p.x + 2 || x + 2 >= r);
                        if corner {
                            none
                        } else if left {
                            on(if upper(y) { "◢" } else { "◥" })
                        } else if right {
                            on(if upper(y) { "◣" } else { "◤" })
                        } else if top {
                            on(if taper { if x <= p.x + 2 { "▗" } else { "▖" } } else { "▄" })
                        } else {
                            on(if taper { if x <= p.x + 2 { "▝" } else { "▘" } } else { "▀" })
                        }
                    }
                    // Slanted sides (input / output).
                    Symbol::Io => match (left, right, top, bottom) {
                        (true, _, true, _) | (_, true, _, true) => none,
                        (true, _, _, true) => on("▀"),
                        (_, true, true, _) => on("▄"),
                        (true, _, _, _) => on("◢"),
                        (_, true, _, _) => on("◤"),
                        (_, _, true, _) => on("▄"),
                        _ => on("▀"),
                    },
                    // Flat on the left, round on the right.
                    Symbol::Delay => match (left, right, top, bottom) {
                        (true, _, true, _) => on("▗"),
                        (true, _, _, true) => on("▝"),
                        (_, true, true, _) | (_, true, _, true) => none,
                        (true, _, _, _) => on("▐"),
                        (_, true, _, _) => on(if mids <= 1 { "▌" } else if upper(y) { "▙" } else { "▛" }),
                        (_, _, true, _) => on(if x == r - 1 { "▖" } else { "▄" }),
                        _ => on(if x == r - 1 { "▘" } else { "▀" }),
                    },
                    // A card with soft corners; a few with marks of their own.
                    _ => match (left, right, top, bottom) {
                        (true, _, true, _) => on(if symbol == Symbol::Manual { "▁" } else { "▗" }),
                        (_, true, true, _) => {
                            if symbol == Symbol::Note {
                                none
                            } else {
                                on("▖")
                            }
                        }
                        (true, _, _, true) => on("▝"),
                        (_, true, _, true) => on("▘"),
                        // Manual input: the top edge slopes up to the right.
                        (_, _, true, _) if symbol == Symbol::Manual && x < p.x + p.w / 2 => on("▁"),
                        (_, _, true, _) if symbol == Symbol::Note && x == r - 1 => on("▖"),
                        (_, _, true, _) => on("▄"),
                        // A document's wavy bottom.
                        (_, _, _, true) if symbol == Symbol::Document && (x - p.x) % 4 >= 2 => on("▔"),
                        (_, _, _, true) => on("▀"),
                        (true, _, _, _) => on("▐"),
                        _ => on("▌"),
                    },
                }
            };
            if !sym.is_empty() {
                put(buf, x, y, sym, st);
            }
        }
    }
    // Marks inside: a subprocess's double bars, a data store's lid, a note's fold.
    let mark = Style::default().fg(line).bg(fill);
    match symbol {
        Symbol::Subprocess => {
            for y in p.y + 1..b {
                put(buf, p.x + 1, y, "│", mark);
                put(buf, r - 1, y, "│", mark);
            }
        }
        Symbol::Data => {
            for x in p.x + 1..r {
                put(buf, x, p.y + 1, "‿", mark);
            }
        }
        Symbol::Note => put(buf, r - 1, p.y + 1, "◥", Style::default().fg(bg).bg(fill)),
        _ => {}
    }
    let first_row = p.y + 1 + u16::from(symbol == Symbol::Data);
    for (i, ln) in p.lines.iter().enumerate() {
        let y = first_row + i as u16;
        let tx = p.x + (p.w - ln.chars().count() as u16) / 2;
        let style = match (symbol, i) {
            (Symbol::Connector, _) | (Symbol::Gate(_), 1) => Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            (Symbol::Gate(_), 0) => Style::default().fg(line),
            (Symbol::Note, _) => Style::default().fg(t.text_muted).add_modifier(Modifier::ITALIC),
            (_, 0) => Style::default().fg(t.text_muted),
            _ => Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        };
        for (j, ch) in ln.chars().enumerate() {
            put(buf, tx + j as u16, y, &ch.to_string(), style.bg(fill));
        }
    }
}

/// The chart as SVG in the theme's colours, `cols` x `rows` cells big, the layout centred.
pub fn svg(chart: &Flowchart, t: &Theme, cols: u16, rows: u16) -> String {
    let l = layout(chart);
    let bg = t.background;
    let ox = cols.saturating_sub(l.width) / 2;
    let oy = rows.saturating_sub(l.height) / 2;
    let px = |x: u16| (x + ox) as f32 * CW;
    let py = |y: u16| (y + oy) as f32 * CH;
    let (line, text, muted) = (hex(t.border_subtle), hex(t.text), hex(t.text_muted));
    let mut s = svg_open(cols, rows, t.text_muted);
    for (a, b, label) in &l.arrows {
        let ((x0, y0), bend, (x1, y1)) = route(&l, *a, *b);
        let (path, lx, ly) = match l.direction {
            FlowDirection::Right => {
                let (fx0, fy0, fm, fx1, fy1) = (px(x0), py(y0) + CH / 2.0, px(bend) + CW / 2.0, px(x1) - 2.0, py(y1) + CH / 2.0);
                let r = 10.0f32.min((fy1 - fy0).abs() / 2.0);
                let path = if (fy1 - fy0).abs() < 1.0 {
                    format!("M{fx0} {fy0} L{fx1} {fy1}")
                } else {
                    let dir = if fy1 > fy0 { 1.0 } else { -1.0 };
                    format!("M{fx0} {fy0} L{} {fy0} Q{fm} {fy0} {fm} {} L{fm} {} Q{fm} {fy1} {} {fy1} L{fx1} {fy1}", fm - r, fy0 + dir * r, fy1 - dir * r, fm + r)
                };
                (path, fm + (fx1 - fm) / 2.0, fy1)
            }
            FlowDirection::Down => {
                let (fx0, fy0, fm, fx1, fy1) = (px(x0) + CW / 2.0, py(y0), py(bend) + CH / 2.0, px(x1) + CW / 2.0, py(y1) - 2.0);
                let r = 10.0f32.min((fx1 - fx0).abs() / 2.0);
                let path = if (fx1 - fx0).abs() < 1.0 {
                    format!("M{fx0} {fy0} L{fx1} {fy1}")
                } else {
                    let dir = if fx1 > fx0 { 1.0 } else { -1.0 };
                    format!("M{fx0} {fy0} L{fx0} {} Q{fx0} {fm} {} {fm} L{} {fm} Q{fx1} {fm} {fx1} {} L{fx1} {fy1}", fm - r, fx0 + dir * r, fx1 - dir * r, fm + r)
                };
                (path, fx1 + 30.0, fm + (fy1 - fm) / 2.0)
            }
        };
        s += &format!(r#"<path d="{path}" fill="none" stroke="{muted}" stroke-width="2" marker-end="url(#arrow)"/>"#);
        if let Some(lb) = label {
            let lw = (lb.chars().count() as f32 + 2.0) * CW;
            let lx = lx - lw / 2.0;
            s += &format!(
                r#"<rect x="{lx}" y="{}" width="{lw}" height="22" rx="11" fill="{}" stroke="{line}" stroke-width="1.5"/><text x="{}" y="{}" fill="{text}" text-anchor="middle">{}</text>"#,
                ly - 11.0,
                hex(t.surface),
                lx + lw / 2.0,
                ly + 6.0,
                esc(lb)
            );
        }
    }
    for p in &l.nodes {
        let (fill, stroke) = colours(p, t, bg);
        let (fill, stroke) = (hex(fill), hex(stroke));
        let (x, y, w, h) = (px(p.x) + 2.0, py(p.y) + 4.0, p.w as f32 * CW - 4.0, p.h as f32 * CH - 8.0);
        let (cx, cy, r, b) = (x + w / 2.0, y + h / 2.0, x + w, y + h);
        let st = format!(r#"fill="{fill}" stroke="{stroke}" stroke-width="2.5" stroke-linejoin="round""#);
        s += &match p.symbol {
            Symbol::Start | Symbol::End | Symbol::Connector => format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{}" {st}/>"#, h / 2.0),
            Symbol::Decision => format!(r#"<path d="M{cx} {} L{} {cy} L{cx} {} L{} {cy} Z" {st}/>"#, y - 6.0, r + 6.0, b + 6.0, x - 6.0),
            Symbol::Prep => format!(r#"<path d="M{} {y} L{} {y} L{r} {cy} L{} {b} L{} {b} L{x} {cy} Z" {st}/>"#, x + 14.0, r - 14.0, r - 14.0, x + 14.0),
            Symbol::Io => format!(r#"<path d="M{} {y} L{r} {y} L{} {b} L{x} {b} Z" {st}/>"#, x + 16.0, r - 16.0),
            Symbol::Manual => format!(r#"<path d="M{x} {} L{r} {y} L{r} {b} L{x} {b} Z" {st}/>"#, y + 14.0),
            Symbol::Subprocess => format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="4" {st}/><path d="M{} {y} V{b} M{} {y} V{b}" stroke="{stroke}" stroke-width="2"/>"#,
                x + 10.0,
                r - 10.0
            ),
            Symbol::Document => format!(r#"<path d="M{x} {y} H{r} V{} C{} {} {} {} {x} {} Z" {st}/>"#, b - 8.0, r - w * 0.3, b + 8.0, x + w * 0.3, b - 18.0, b - 8.0),
            Symbol::Data => {
                let (ry, rx) = (9.0, w / 2.0);
                format!(
                    r#"<path d="M{x} {} V{} A{rx} {ry} 0 0 0 {r} {} V{} A{rx} {ry} 0 0 0 {x} {} Z" {st}/><path d="M{x} {} A{rx} {ry} 0 0 0 {r} {}" fill="none" stroke="{stroke}" stroke-width="2"/>"#,
                    y + ry,
                    b - ry,
                    b - ry,
                    y + ry,
                    y + ry,
                    y + ry,
                    y + ry
                )
            }
            Symbol::Delay => format!(r#"<path d="M{x} {y} H{} A{} {} 0 0 1 {} {b} H{x} Z" {st}/>"#, r - h / 2.0, h / 2.0, h / 2.0, r - h / 2.0),
            Symbol::Note => format!(
                r#"<path d="M{x} {y} H{} L{r} {} V{b} H{x} Z" {st} stroke-dasharray="6 4"/><path d="M{} {y} V{} H{r}" fill="none" stroke="{stroke}" stroke-width="1.5"/>"#,
                r - 14.0,
                y + 14.0,
                r - 14.0,
                y + 14.0
            ),
            Symbol::Gate(g) => {
                let inverted = matches!(g, Gate::Not | Gate::Nand | Gate::Nor | Gate::Xnor);
                let mut out = format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="4" {st}/>"#);
                if inverted {
                    let (bx, by) = match l.direction {
                        FlowDirection::Right => (r + 5.0, cy),
                        FlowDirection::Down => (cx, b + 5.0),
                    };
                    out += &format!(r#"<circle cx="{bx}" cy="{by}" r="5" fill="{}" stroke="{stroke}" stroke-width="2"/>"#, hex(bg));
                }
                out
            }
            Symbol::Process => format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" {st}/>"#),
        };
        let top = cy - (p.lines.len() as f32 - 1.0) * CH * 0.8 / 2.0 + 6.0 + if p.symbol == Symbol::Data { 6.0 } else { 0.0 };
        for (i, ln) in p.lines.iter().enumerate() {
            let shown = match p.symbol {
                Symbol::Gate(_) => ln.trim_end_matches(" o").to_string(),
                _ => ln.clone(),
            };
            let (fill, weight) = match (p.symbol, i) {
                (Symbol::Connector, _) | (Symbol::Gate(_), 1) => (text.clone(), r#" font-weight="bold""#),
                (_, 0) => (muted.clone(), ""),
                _ => (text.clone(), ""),
            };
            s += &format!(r#"<text x="{cx}" y="{}" fill="{fill}" text-anchor="middle"{weight}>{}</text>"#, top + i as f32 * CH * 0.8, esc(&shown));
        }
    }
    s += "</svg>";
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Arrow, FlowNode};

    fn chart(direction: FlowDirection) -> Flowchart {
        let n = |id: &str, symbol| FlowNode { id: id.into(), text: id.into(), symbol, refs: vec![], changed: false };
        let a = |f: &str, t: &str| Arrow { from: f.into(), to: t.into(), label: None };
        Flowchart {
            direction,
            nodes: vec![n("n1", Symbol::Start), n("n2", Symbol::Decision), n("n3", Symbol::Process), n("n4", Symbol::Process)],
            arrows: vec![a("n1", "n2"), a("n2", "n3"), a("n2", "n4")],
        }
    }

    #[test]
    fn right_ranks_are_columns_and_siblings_stack() {
        let l = layout(&chart(FlowDirection::Right));
        assert!(l.nodes[0].x < l.nodes[1].x && l.nodes[1].x < l.nodes[2].x);
        assert_eq!(l.nodes[2].x, l.nodes[3].x);
        assert!(l.nodes[2].y + l.nodes[2].h <= l.nodes[3].y);
    }

    #[test]
    fn down_ranks_are_rows_and_siblings_sit_side_by_side() {
        let l = layout(&chart(FlowDirection::Down));
        assert!(l.nodes[0].y < l.nodes[1].y && l.nodes[1].y < l.nodes[2].y);
        assert_eq!(l.nodes[2].y, l.nodes[3].y);
        assert!(l.nodes[2].x + l.nodes[2].w <= l.nodes[3].x);
    }

    #[test]
    fn every_svg_parses() {
        let t = Theme::builtin("dark").unwrap();
        for c in [chart(FlowDirection::Right), chart(FlowDirection::Down), crate::debug::sample_flow_gallery(), crate::debug::sample_flowchart()] {
            let s = svg(&c, &t, 140, 40);
            assert!(resvg::usvg::Tree::from_str(&s, &resvg::usvg::Options::default()).is_ok());
        }
    }
}
