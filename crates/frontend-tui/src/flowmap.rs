//! A flowchart laid out on the terminal's cell grid (journal#136): nodes in columns by their
//! distance from the start, arrows routed right then down or up then right. One layout serves
//! both renderings, so they agree cell for cell: an SVG (the picture in graphics terminals,
//! vector per journal#140) and box-drawing characters (the text fallback).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::model::{FlowMap, NodeShape};
use crate::theme::Theme;

/// Pixels per cell in the SVG's coordinates; the picture is scaled to the real cell size.
const CW: f32 = 12.0;
const CH: f32 = 26.0;
/// Empty columns between two ranks (room for the arrow and its label).
const GAP: u16 = 12;
/// Rows between nodes stacked in one rank.
const VGAP: u16 = 1;

/// A node's box in cells, relative to the layout's origin.
#[derive(Debug, Clone)]
struct Placed {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
    lines: Vec<String>,
    shape: NodeShape,
    changed: bool,
}

#[derive(Debug, Clone)]
pub struct Layout {
    nodes: Vec<Placed>,
    /// (from, to, label) by node index.
    edges: Vec<(usize, usize, Option<String>)>,
    pub width: u16,
    pub height: u16,
}

/// Lines of a node: its id, then its text wrapped to `max` columns.
fn node_lines(id: &str, text: &str, max: usize) -> Vec<String> {
    let mut out = vec![id.to_string()];
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > max {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn layout(map: &FlowMap) -> Layout {
    let n = map.nodes.len();
    let index = |id: &str| map.nodes.iter().position(|x| x.id == id);
    let edges: Vec<(usize, usize, Option<String>)> =
        map.edges.iter().filter_map(|e| Some((index(&e.from)?, index(&e.to)?, e.label.clone()))).collect();
    // Rank: longest path from a node without incoming edges (cycles cut by the iteration cap).
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        for &(a, b, _) in &edges {
            if rank[b] < rank[a] + 1 && rank[a] + 1 < n {
                rank[b] = rank[a] + 1;
            }
        }
    }
    let ranks = rank.iter().copied().max().map_or(0, |r| r + 1);
    let mut placed: Vec<Placed> = map
        .nodes
        .iter()
        .map(|nd| {
            let lines = node_lines(&nd.id, &nd.text, 18);
            let text_w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(1) as u16;
            let (w, h) = match nd.shape {
                NodeShape::Box => (text_w + 4, lines.len() as u16 + 2),
                // A diamond needs room around its text.
                NodeShape::Decision => (text_w + 8, lines.len() as u16 + 2),
            };
            Placed { x: 0, y: 0, w, h, lines, shape: nd.shape, changed: nd.changed }
        })
        .collect();
    // Columns: as wide as their widest node; nodes of one rank stacked, centred on the tallest
    // column.
    let mut col_x = Vec::new();
    let mut x = 0u16;
    let mut col_h = Vec::new();
    for r in 0..ranks {
        let members: Vec<usize> = (0..n).filter(|&i| rank[i] == r).collect();
        let w = members.iter().map(|&i| placed[i].w).max().unwrap_or(0);
        col_x.push((x, w));
        x += w + GAP;
        let h: u16 = members.iter().map(|&i| placed[i].h).sum::<u16>() + VGAP * members.len().saturating_sub(1) as u16;
        col_h.push(h);
    }
    let height = col_h.iter().copied().max().unwrap_or(0);
    for r in 0..ranks {
        let (cx, cw) = col_x[r];
        let mut y = (height - col_h[r]) / 2;
        for i in (0..n).filter(|&i| rank[i] == r) {
            let p = &mut placed[i];
            p.x = cx + (cw - p.w) / 2;
            p.y = y;
            y += p.h + VGAP;
        }
    }
    Layout { nodes: placed, edges, width: x.saturating_sub(GAP), height }
}

/// The arrow from node `a` to node `b` as cell points: out of a's right middle, across to the
/// middle of the gap, down or up, into b's left middle.
fn route(l: &Layout, a: usize, b: usize) -> (u16, u16, u16, u16, u16) {
    let (pa, pb) = (&l.nodes[a], &l.nodes[b]);
    let (x0, y0) = (pa.x + pa.w, pa.y + pa.h / 2);
    let (x1, y1) = (pb.x, pb.y + pb.h / 2);
    let mid = x0 + (x1.saturating_sub(x0)) / 2;
    (x0, y0, mid, x1, y1)
}

fn hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => "#888888".into(),
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The flowchart as SVG in the theme's colours, `cols` x `rows` cells big, the layout centred.
pub fn svg(map: &FlowMap, t: &Theme, cols: u16, rows: u16) -> String {
    let l = layout(map);
    let ox = cols.saturating_sub(l.width) / 2;
    let oy = rows.saturating_sub(l.height) / 2;
    let px = |x: u16| (x + ox) as f32 * CW;
    let py = |y: u16| (y + oy) as f32 * CH;
    let (w, h) = (cols as f32 * CW, rows as f32 * CH);
    let (line, text, muted, accent) = (hex(t.border_subtle), hex(t.text), hex(t.text_muted), hex(t.accent_line));
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="DejaVu Sans Mono, monospace" font-size="17">"#
    );
    s += &format!(r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L10 5 L0 10 z" fill="{muted}"/></marker></defs>"#);
    for (a, b, label) in &l.edges {
        let (x0, y0, mid, x1, y1) = route(&l, *a, *b);
        let (fx0, fy0, fm, fx1, fy1) = (px(x0), py(y0) + CH / 2.0, px(mid) + CW / 2.0, px(x1) - 2.0, py(y1) + CH / 2.0);
        let r = 10.0f32.min((fy1 - fy0).abs() / 2.0);
        let path = if (fy1 - fy0).abs() < 1.0 {
            format!("M{fx0} {fy0} L{fx1} {fy1}")
        } else {
            let dir = if fy1 > fy0 { 1.0 } else { -1.0 };
            format!(
                "M{fx0} {fy0} L{} {fy0} Q{fm} {fy0} {fm} {} L{fm} {} Q{fm} {fy1} {} {fy1} L{fx1} {fy1}",
                fm - r,
                fy0 + dir * r,
                fy1 - dir * r,
                fm + r
            )
        };
        s += &format!(r#"<path d="{path}" fill="none" stroke="{muted}" stroke-width="2" marker-end="url(#arrow)"/>"#);
        if let Some(lb) = label {
            let lw = (lb.chars().count() as f32 + 2.0) * CW;
            let lx = fm + (fx1 - fm) / 2.0 - lw / 2.0;
            s += &format!(
                r#"<rect x="{lx}" y="{}" width="{lw}" height="{}" rx="9" fill="{}" stroke="{line}" stroke-width="1.5"/><text x="{}" y="{}" fill="{text}" text-anchor="middle">{}</text>"#,
                fy1 - 13.0,
                26.0,
                hex(t.surface),
                lx + lw / 2.0,
                fy1 + 6.0,
                esc(lb)
            );
        }
    }
    for p in &l.nodes {
        let (x, y, w, h) = (px(p.x), py(p.y) + 4.0, p.w as f32 * CW, p.h as f32 * CH - 8.0);
        let (stroke, fill) = if p.changed { (accent.clone(), hex(mix(t.accent_line, t.background, 0.22))) } else { (line.clone(), hex(t.surface)) };
        match p.shape {
            NodeShape::Box => {
                s += &format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" fill="{fill}" stroke="{stroke}" stroke-width="2.5"/>"#);
            }
            NodeShape::Decision => {
                let (cx, cy) = (x + w / 2.0, y + h / 2.0);
                s += &format!(
                    r#"<path d="M{cx} {} L{} {cy} L{cx} {} L{} {cy} Z" fill="{fill}" stroke="{accent}" stroke-width="2.5" stroke-linejoin="round"/>"#,
                    y - 4.0,
                    x + w,
                    y + h + 4.0,
                    x
                );
            }
        }
        let top = y + h / 2.0 - (p.lines.len() as f32 - 1.0) * CH * 0.8 / 2.0 + 6.0;
        for (i, ln) in p.lines.iter().enumerate() {
            s += &format!(
                r#"<text x="{}" y="{}" fill="{text}" text-anchor="middle">{}</text>"#,
                x + w / 2.0,
                top + i as f32 * CH * 0.8,
                esc(ln)
            );
        }
    }
    s += "</svg>";
    s
}

fn mix(a: Color, b: Color, alpha: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 * alpha + y as f32 * (1.0 - alpha)).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => b,
    }
}

/// The same flowchart in box-drawing characters, centred in `area`: the text fallback.
pub fn draw_text(buf: &mut Buffer, area: Rect, map: &FlowMap, t: &Theme, bg: Color) {
    let l = layout(map);
    let ox = area.x + area.width.saturating_sub(l.width) / 2;
    let oy = area.y + area.height.saturating_sub(l.height) / 2;
    let put = |buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style| {
        let pos = (ox + x, oy + y);
        if area.contains(pos.into()) {
            buf[pos].set_symbol(s).set_style(st.bg(bg));
        }
    };
    let edge = Style::default().fg(t.text_muted);
    for (a, b, label) in &l.edges {
        let (x0, y0, mid, x1, y1) = route(&l, *a, *b);
        for x in x0..mid {
            put(buf, x, y0, "─", edge);
        }
        if y0 != y1 {
            let (lo, hi) = (y0.min(y1), y0.max(y1));
            for y in lo + 1..hi {
                put(buf, mid, y, "│", edge);
            }
            put(buf, mid, y0, if y1 > y0 { "╮" } else { "╯" }, edge);
            put(buf, mid, y1, if y1 > y0 { "╰" } else { "╭" }, edge);
        } else {
            put(buf, mid, y0, "─", edge);
        }
        for x in mid + 1..x1.saturating_sub(1) {
            put(buf, x, y1, "─", edge);
        }
        put(buf, x1.saturating_sub(1), y1, "▶", edge);
        if let Some(lb) = label {
            let lx = mid + 2;
            for (i, ch) in lb.chars().enumerate() {
                put(buf, lx + i as u16, y1, &ch.to_string(), Style::default().fg(t.text));
            }
        }
    }
    for p in &l.nodes {
        let colour = if p.changed || p.shape == NodeShape::Decision { t.accent_line } else { t.border_subtle };
        let st = Style::default().fg(colour);
        let (r, b) = (p.x + p.w - 1, p.y + p.h - 1);
        let (tl, tr, bl, br, hz, vt) = match p.shape {
            NodeShape::Box => ("╭", "╮", "╰", "╯", "─", "│"),
            NodeShape::Decision => ("╱", "╲", "╲", "╱", "─", "◇"),
        };
        for x in p.x..=r {
            for y in p.y..=b {
                let s = match (x == p.x, x == r, y == p.y, y == b) {
                    (true, _, true, _) => tl,
                    (_, true, true, _) => tr,
                    (true, _, _, true) => bl,
                    (_, true, _, true) => br,
                    (_, _, true, _) | (_, _, _, true) => hz,
                    (true, _, _, _) | (_, true, _, _) => vt,
                    _ => " ",
                };
                put(buf, x, y, s, st);
            }
        }
        let fill = if p.changed { mix(t.accent_line, bg, 0.22) } else { bg };
        for (i, ln) in p.lines.iter().enumerate() {
            let y = p.y + 1 + i as u16;
            for x in p.x + 1..r {
                let pos = (ox + x, oy + y);
                if area.contains(pos.into()) {
                    buf[pos].set_symbol(" ").set_bg(fill);
                }
            }
            let tx = p.x + (p.w - ln.chars().count() as u16) / 2;
            let style = if i == 0 { Style::default().fg(t.text_muted) } else { Style::default().fg(t.text).add_modifier(Modifier::BOLD) };
            for (j, ch) in ln.chars().enumerate() {
                let pos = (ox + tx + j as u16, oy + y);
                if area.contains(pos.into()) {
                    buf[pos].set_symbol(&ch.to_string()).set_style(style).set_bg(fill);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowEdge, FlowNode};

    fn map() -> FlowMap {
        let n = |id: &str, text: &str, shape| FlowNode { id: id.into(), text: text.into(), shape, changed: false };
        let e = |a: &str, b: &str| FlowEdge { from: a.into(), to: b.into(), label: None };
        FlowMap {
            nodes: vec![n("n1", "Start", NodeShape::Box), n("n2", "OK?", NodeShape::Decision), n("n3", "A", NodeShape::Box), n("n4", "B", NodeShape::Box)],
            edges: vec![e("n1", "n2"), e("n2", "n3"), e("n2", "n4")],
        }
    }

    #[test]
    fn ranks_go_left_to_right_and_siblings_stack() {
        let l = layout(&map());
        assert!(l.nodes[0].x < l.nodes[1].x && l.nodes[1].x < l.nodes[2].x);
        assert_eq!(l.nodes[2].x, l.nodes[3].x);
        assert!(l.nodes[2].y + l.nodes[2].h <= l.nodes[3].y);
    }

    #[test]
    fn the_svg_parses() {
        let t = Theme::builtin("dark").unwrap();
        let s = svg(&map(), &t, 80, 10);
        assert!(resvg::usvg::Tree::from_str(&s, &resvg::usvg::Options::default()).is_ok());
    }
}
