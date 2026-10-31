//! Maps drawn on the terminal's cell grid (journal#136): mind maps (a tree of thoughts, every
//! node kind of the journal's mind maps) and flowcharts (a block diagram by the canon, logic
//! gates included). One layout serves both renderings, so they agree cell for cell: an SVG
//! (the picture in graphics terminals, vector per journal#140) and block and box-drawing
//! characters (the text form, the main one: journal#227).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::model::{FlowMap, Gate, MapKind, NodeShape, NodeStatus};
use crate::theme::Theme;

/// Pixels per cell in the SVG's coordinates; the picture is scaled to the real cell size.
const CW: f32 = 12.0;
const CH: f32 = 26.0;
/// Columns between two ranks of a flowchart (room for the arrow and its label).
const GAP: u16 = 8;
/// Rows between nodes stacked in one rank or between siblings of a mind map.
const VGAP: u16 = 1;
/// Widest text line in a flowchart node.
const TEXT_MAX: usize = 16;
/// Widest mind-map node text before an ellipsis.
const MIND_MAX: usize = 34;

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
    status: NodeStatus,
    refs: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    kind: MapKind,
    nodes: Vec<Placed>,
    /// (from, to, label, cross link) by node index.
    edges: Vec<(usize, usize, Option<String>, bool)>,
    /// Mind maps: the row where the list of cross links starts.
    links_y: u16,
    pub width: u16,
    pub height: u16,
}

fn wrap(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
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

fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
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

/// A mind-map kind's glyph and colour.
fn mind_style(shape: NodeShape, status: NodeStatus, t: &Theme) -> (&'static str, Color) {
    match shape {
        NodeShape::Topic => ("◆", t.accent),
        NodeShape::Goal => ("◎", t.accent),
        NodeShape::Choice => ("◈", t.agent),
        NodeShape::Fact => ("●", t.success),
        NodeShape::Hypothesis => ("◐", t.warning),
        NodeShape::Question => ("?", t.info),
        NodeShape::Task if status == NodeStatus::Done => ("☑", t.success),
        NodeShape::Task => ("☐", t.text),
        NodeShape::Idea => ("✦", t.warning),
        NodeShape::DeadEnd => ("✕", t.error),
        _ => ("✎", t.text_muted),
    }
}

pub fn layout(map: &FlowMap) -> Layout {
    match map.kind {
        MapKind::Flowchart => flow_layout(map),
        MapKind::Mindmap => mind_layout(map),
    }
}

fn index_edges(map: &FlowMap) -> Vec<(usize, usize, Option<String>, bool)> {
    let index = |id: &str| map.nodes.iter().position(|x| x.id == id);
    map.edges.iter().filter_map(|e| Some((index(&e.from)?, index(&e.to)?, e.label.clone(), e.link))).collect()
}

fn flow_layout(map: &FlowMap) -> Layout {
    let n = map.nodes.len();
    let edges = index_edges(map);
    // Rank: longest path from a node without incoming edges (cycles cut by the iteration cap).
    let mut rank = vec![0usize; n];
    for _ in 0..n {
        for &(a, b, _, _) in &edges {
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
            let lines = match nd.shape {
                NodeShape::Connector => vec![nd.text.clone()],
                NodeShape::Gate(g) => {
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
            let extra = match nd.shape {
                NodeShape::Decision => 8,
                NodeShape::Start | NodeShape::End | NodeShape::Prep | NodeShape::Io | NodeShape::Manual | NodeShape::Subprocess => 6,
                NodeShape::Delay | NodeShape::Connector => 5,
                _ => 4,
            };
            let rows = lines.len() as u16 + 2 + u16::from(nd.shape == NodeShape::Data);
            Placed { x: 0, y: 0, w: text_w + extra, h: rows, lines, shape: nd.shape, changed: nd.changed, status: nd.status, refs: nd.refs.clone() }
        })
        .collect();
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
    Layout { kind: MapKind::Flowchart, nodes: placed, edges, links_y: height, width: x.saturating_sub(GAP), height }
}

fn mind_layout(map: &FlowMap) -> Layout {
    let n = map.nodes.len();
    let edges = index_edges(map);
    let mut placed: Vec<Placed> = map
        .nodes
        .iter()
        .map(|nd| {
            let refs: String = nd.refs.iter().map(|r| format!("  #{r}")).collect();
            let label = format!("{}{}", fit(&nd.text, MIND_MAX), refs);
            // Pill: cap, glyph, space, label, space, cap.
            let w = label.chars().count() as u16 + 5;
            Placed { x: 0, y: 0, w, h: 1, lines: vec![label], shape: nd.shape, changed: nd.changed, status: nd.status, refs: nd.refs.clone() }
        })
        .collect();
    // The tree: non-link edges from a parent to a child; nodes without a parent are roots.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_parent = vec![false; n];
    for &(a, b, _, link) in &edges {
        if !link && !has_parent[b] && a != b {
            children[a].push(b);
            has_parent[b] = true;
        }
    }
    let roots: Vec<usize> = (0..n).filter(|&i| !has_parent[i]).collect();
    let mut depth = vec![0usize; n];
    fn visit(i: usize, d: usize, children: &[Vec<usize>], depth: &mut [usize], seen: &mut [bool]) {
        if seen[i] {
            return;
        }
        seen[i] = true;
        depth[i] = d;
        for &c in &children[i] {
            visit(c, d + 1, children, depth, seen);
        }
    }
    let mut seen = vec![false; n];
    for &r in &roots {
        visit(r, 0, &children, &mut depth, &mut seen);
    }
    // Columns by depth: as wide as the widest node of the depth before, then room for the
    // connectors.
    let max_d = depth.iter().copied().max().unwrap_or(0);
    let mut col_x = vec![0u16; max_d + 1];
    for d in 1..=max_d {
        let w = (0..n).filter(|&i| depth[i] == d - 1).map(|i| placed[i].w).max().unwrap_or(0);
        col_x[d] = col_x[d - 1] + w + 6;
    }
    // Rows: leaves one after another, a parent centred on its children.
    fn place(i: usize, children: &[Vec<usize>], placed: &mut [Placed], next: &mut u16) -> u16 {
        if children[i].is_empty() {
            placed[i].y = *next;
            *next += 1 + VGAP;
            return placed[i].y;
        }
        let ys: Vec<u16> = children[i].clone().into_iter().map(|c| place(c, children, placed, next)).collect();
        placed[i].y = (ys[0] + ys[ys.len() - 1]) / 2;
        placed[i].y
    }
    let mut next = 0u16;
    for &r in &roots {
        place(r, &children, &mut placed, &mut next);
    }
    for i in 0..n {
        placed[i].x = col_x[depth[i]];
    }
    let tree_h = next.saturating_sub(VGAP);
    let links = edges.iter().filter(|e| e.3).count() as u16;
    let width = placed.iter().map(|p| p.x + p.w).max().unwrap_or(0).max(if links > 0 { 40 } else { 0 });
    let links_y = tree_h + u16::from(links > 0);
    Layout { kind: MapKind::Mindmap, nodes: placed, edges, links_y, width, height: links_y + links }
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

fn mix(a: Color, b: Color, alpha: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 * alpha + y as f32 * (1.0 - alpha)).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => b,
    }
}

/// A flowchart symbol's fill and line colour.
fn flow_colours(p: &Placed, t: &Theme, bg: Color) -> (Color, Color) {
    if p.changed {
        return (mix(t.accent_line, bg, 0.30), t.accent_line);
    }
    match p.shape {
        NodeShape::Start => (mix(t.success, bg, 0.16), t.success),
        NodeShape::End => (mix(t.error, bg, 0.14), t.error),
        NodeShape::Decision => (mix(t.accent_line, bg, 0.14), t.accent_line),
        NodeShape::Gate(_) => (mix(t.info, bg, 0.14), t.info),
        NodeShape::Io | NodeShape::Manual => (mix(t.info, bg, 0.10), t.info),
        NodeShape::Data | NodeShape::Document => (mix(t.agent, bg, 0.12), t.agent),
        NodeShape::Note | NodeShape::Connector => (mix(t.text_muted, bg, 0.12), t.text_muted),
        _ => (t.surface_raised, t.border_subtle),
    }
}

/// A mind-map node's fill and line colour.
fn mind_colours(p: &Placed, t: &Theme, bg: Color) -> (Color, Color) {
    let (_, c) = mind_style(p.shape, p.status, t);
    if p.changed {
        return (mix(t.accent_line, bg, 0.30), t.accent_line);
    }
    match p.shape {
        NodeShape::Topic => (mix(c, bg, 0.32), c),
        NodeShape::Remark | NodeShape::Task => (t.surface_raised, t.border_subtle),
        _ => (mix(c, bg, 0.16), c),
    }
}

/// The map as SVG in the theme's colours, `cols` x `rows` cells big, the layout centred.
pub fn svg(map: &FlowMap, t: &Theme, cols: u16, rows: u16) -> String {
    let l = layout(map);
    let bg = t.background;
    let ox = cols.saturating_sub(l.width) / 2;
    let oy = rows.saturating_sub(l.height) / 2;
    let px = |x: u16| (x + ox) as f32 * CW;
    let py = |y: u16| (y + oy) as f32 * CH;
    let (w, h) = (cols as f32 * CW, rows as f32 * CH);
    let (line, text, muted) = (hex(t.border_subtle), hex(t.text), hex(t.text_muted));
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="DejaVu Sans Mono, monospace" font-size="17">"#
    );
    s += &format!(r#"<defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L10 5 L0 10 z" fill="{muted}"/></marker></defs>"#);
    if l.kind == MapKind::Mindmap {
        // Branches: a curve from the parent's right end to the child's left end.
        for (a, b, _, link) in &l.edges {
            if *link {
                continue;
            }
            let (pa, pb) = (&l.nodes[*a], &l.nodes[*b]);
            let (x0, y0) = (px(pa.x + pa.w) - 2.0, py(pa.y) + CH / 2.0);
            let (x1, y1) = (px(pb.x) + 2.0, py(pb.y) + CH / 2.0);
            let m = (x0 + x1) / 2.0;
            s += &format!(r#"<path d="M{x0} {y0} C{m} {y0} {m} {y1} {x1} {y1}" fill="none" stroke="{line}" stroke-width="2"/>"#);
        }
        for p in &l.nodes {
            let (fill, stroke) = mind_colours(p, t, bg);
            let (glyph, gc) = mind_style(p.shape, p.status, t);
            let (x, y, w) = (px(p.x), py(p.y) + 2.0, p.w as f32 * CW);
            let sw = if p.shape == NodeShape::Topic { 2.5 } else { 1.5 };
            s += &format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{}" rx="11" fill="{}" stroke="{}" stroke-width="{sw}"/>"#, CH - 4.0, hex(fill), hex(stroke));
            let dropped = p.status == NodeStatus::Dropped;
            let tc = if dropped || p.status == NodeStatus::Done { muted.clone() } else { text.clone() };
            let deco = if dropped { r#" text-decoration="line-through""# } else { "" };
            let weight = if p.shape == NodeShape::Topic { r#" font-weight="bold""# } else { "" };
            s += &format!(r#"<text x="{}" y="{}" fill="{}">{}</text>"#, x + CW, y + 17.0, hex(gc), esc(glyph));
            s += &format!(r#"<text x="{}" y="{}" fill="{tc}"{deco}{weight}>{}</text>"#, x + CW * 3.0, y + 17.0, esc(&p.lines[0]));
        }
        // Cross links: dashed curves, listed under the tree with their labels.
        for (i, (a, b, label, _)) in l.edges.iter().filter(|e| e.3).enumerate() {
            // Out of the right end of one node and into the right end of the other, bowing
            // out to the right, so the link never crosses node labels.
            let (pa, pb) = (&l.nodes[*a], &l.nodes[*b]);
            let (x0, y0) = (px(pa.x + pa.w), py(pa.y) + CH / 2.0);
            let (x1, y1) = (px(pb.x + pb.w) + 4.0, py(pb.y) + CH / 2.0);
            let bow = x0.max(x1) + 40.0 + (y1 - y0).abs() * 0.15;
            s += &format!(
                r#"<path d="M{x0} {y0} C{bow} {y0} {bow} {y1} {x1} {y1}" fill="none" stroke="{}" stroke-width="1.5" stroke-dasharray="5 4" marker-end="url(#arrow)"/>"#,
                hex(t.agent)
            );
            s += &format!(
                r#"<text x="{}" y="{}" fill="{muted}">⇢ {} → {}  {}</text>"#,
                px(0),
                py(l.links_y + i as u16) + 17.0,
                esc(&map.nodes[*a].id),
                esc(&map.nodes[*b].id),
                esc(label.as_deref().unwrap_or(""))
            );
        }
        s += "</svg>";
        return s;
    }
    for (a, b, label, _) in &l.edges {
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
                r#"<rect x="{lx}" y="{}" width="{lw}" height="22" rx="11" fill="{}" stroke="{line}" stroke-width="1.5"/><text x="{}" y="{}" fill="{text}" text-anchor="middle">{}</text>"#,
                fy1 - 11.0,
                hex(t.surface),
                lx + lw / 2.0,
                fy1 + 6.0,
                esc(lb)
            );
        }
    }
    for p in &l.nodes {
        let (fill, stroke) = flow_colours(p, t, bg);
        let (fill, stroke) = (hex(fill), hex(stroke));
        let (x, y, w, h) = (px(p.x) + 2.0, py(p.y) + 4.0, p.w as f32 * CW - 4.0, p.h as f32 * CH - 8.0);
        let (cx, cy, r, b) = (x + w / 2.0, y + h / 2.0, x + w, y + h);
        let st = format!(r#"fill="{fill}" stroke="{stroke}" stroke-width="2.5" stroke-linejoin="round""#);
        s += &match p.shape {
            NodeShape::Start | NodeShape::End | NodeShape::Connector => {
                format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{}" {st}/>"#, h / 2.0)
            }
            NodeShape::Decision => format!(r#"<path d="M{cx} {} L{} {cy} L{cx} {} L{} {cy} Z" {st}/>"#, y - 6.0, r + 6.0, b + 6.0, x - 6.0),
            NodeShape::Prep => format!(r#"<path d="M{} {y} L{} {y} L{r} {cy} L{} {b} L{} {b} L{x} {cy} Z" {st}/>"#, x + 14.0, r - 14.0, r - 14.0, x + 14.0),
            NodeShape::Io => format!(r#"<path d="M{} {y} L{r} {y} L{} {b} L{x} {b} Z" {st}/>"#, x + 16.0, r - 16.0),
            NodeShape::Manual => format!(r#"<path d="M{x} {} L{r} {y} L{r} {b} L{x} {b} Z" {st}/>"#, y + 14.0),
            NodeShape::Subprocess => format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="4" {st}/><path d="M{} {y} V{b} M{} {y} V{b}" stroke="{stroke}" stroke-width="2"/>"#,
                x + 10.0,
                r - 10.0
            ),
            NodeShape::Document => format!(
                r#"<path d="M{x} {y} H{r} V{} C{} {} {} {} {x} {} Z" {st}/>"#,
                b - 8.0,
                r - w * 0.3,
                b + 8.0,
                x + w * 0.3,
                b - 18.0,
                b - 8.0
            ),
            NodeShape::Data => {
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
            NodeShape::Delay => format!(r#"<path d="M{x} {y} H{} A{} {} 0 0 1 {} {b} H{x} Z" {st}/>"#, r - h / 2.0, h / 2.0, h / 2.0, r - h / 2.0),
            NodeShape::Note => format!(
                r#"<path d="M{x} {y} H{} L{r} {} V{b} H{x} Z" {st} stroke-dasharray="6 4"/><path d="M{} {y} V{} H{r}" fill="none" stroke="{stroke}" stroke-width="1.5"/>"#,
                r - 14.0,
                y + 14.0,
                r - 14.0,
                y + 14.0
            ),
            NodeShape::Gate(g) => {
                let inverted = matches!(g, Gate::Not | Gate::Nand | Gate::Nor | Gate::Xnor);
                let mut out = format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="4" {st}/>"#);
                if inverted {
                    out += &format!(r#"<circle cx="{}" cy="{cy}" r="5" fill="{}" stroke="{stroke}" stroke-width="2"/>"#, r + 5.0, hex(bg));
                }
                out
            }
            _ => format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="9" {st}/>"#),
        };
        let top = cy - (p.lines.len() as f32 - 1.0) * CH * 0.8 / 2.0 + 6.0 + if p.shape == NodeShape::Data { 6.0 } else { 0.0 };
        for (i, ln) in p.lines.iter().enumerate() {
            let shown = match p.shape {
                NodeShape::Gate(_) => ln.trim_end_matches(" o").to_string(),
                _ => ln.clone(),
            };
            let (fill, weight) = match (p.shape, i) {
                (NodeShape::Connector, _) | (NodeShape::Gate(_), 1) => (text.clone(), r#" font-weight="bold""#),
                (_, 0) => (muted.clone(), ""),
                _ => (text.clone(), ""),
            };
            s += &format!(r#"<text x="{cx}" y="{}" fill="{fill}" text-anchor="middle"{weight}>{}</text>"#, top + i as f32 * CH * 0.8, esc(&shown));
        }
    }
    s += "</svg>";
    s
}

type Put<'a> = dyn FnMut(&mut Buffer, u16, u16, &str, Style) + 'a;

/// The map in block and box-drawing characters, centred in `area`: the text form.
pub fn draw_text(buf: &mut Buffer, area: Rect, map: &FlowMap, t: &Theme, bg: Color) {
    let l = layout(map);
    let ox = area.x + area.width.saturating_sub(l.width) / 2;
    let oy = area.y + area.height.saturating_sub(l.height) / 2;
    let mut put = |buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style| {
        let pos = (ox + x, oy + y);
        if area.contains(pos.into()) {
            buf[pos].set_symbol(s).set_style(st);
        }
    };
    match l.kind {
        MapKind::Mindmap => mind_text(buf, &l, map, t, bg, &mut put),
        MapKind::Flowchart => flow_text(buf, &l, t, bg, &mut put),
    }
}

/// The box-drawing character joining the given directions.
fn junction(up: bool, down: bool, left: bool, right: bool) -> &'static str {
    match (up, down, left, right) {
        (false, true, false, true) => "╭",
        (true, false, false, true) => "╰",
        (true, true, false, true) => "├",
        (true, true, true, false) => "┤",
        (true, true, true, true) => "┼",
        (false, true, true, true) => "┬",
        (true, false, true, true) => "┴",
        (false, true, true, false) => "╮",
        (true, false, true, false) => "╯",
        (false, false, _, _) => "─",
        _ => "│",
    }
}

fn mind_text(buf: &mut Buffer, l: &Layout, map: &FlowMap, t: &Theme, bg: Color, put: &mut Put) {
    let edge = Style::default().fg(t.border_subtle).bg(bg);
    // Branches: parent -> spine -> child, joined with box-drawing characters.
    let n = l.nodes.len();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &(a, b, _, link) in &l.edges {
        if !link {
            kids[a].push(b);
        }
    }
    for (a, ks) in kids.iter().enumerate() {
        if ks.is_empty() {
            continue;
        }
        let pa = &l.nodes[a];
        let spine = pa.x + pa.w + 2;
        for x in pa.x + pa.w..spine {
            put(buf, x, pa.y, "─", edge);
        }
        let ys: Vec<u16> = ks.iter().map(|&k| l.nodes[k].y).collect();
        let top = ys.iter().copied().min().unwrap_or(pa.y).min(pa.y);
        let bottom = ys.iter().copied().max().unwrap_or(pa.y).max(pa.y);
        for y in top..=bottom {
            put(buf, spine, y, junction(y > top, y < bottom, y == pa.y, ys.contains(&y)), edge);
        }
        for &k in ks {
            let pk = &l.nodes[k];
            for x in spine + 1..pk.x {
                put(buf, x, pk.y, "─", edge);
            }
        }
    }
    for p in &l.nodes {
        let (fill, _) = mind_colours(p, t, bg);
        let (glyph, gc) = mind_style(p.shape, p.status, t);
        put(buf, p.x, p.y, "▐", Style::default().fg(fill).bg(bg));
        put(buf, p.x + 1, p.y, glyph, Style::default().fg(gc).bg(fill).add_modifier(Modifier::BOLD));
        put(buf, p.x + 2, p.y, " ", Style::default().bg(fill));
        let mut st = Style::default().fg(t.text).bg(fill);
        if p.shape == NodeShape::Topic {
            st = st.add_modifier(Modifier::BOLD);
        }
        match p.status {
            NodeStatus::Done => st = st.fg(t.text_muted),
            NodeStatus::Dropped => st = st.fg(t.text_muted).add_modifier(Modifier::CROSSED_OUT),
            NodeStatus::Open => {}
        }
        let label = &p.lines[0];
        let refs_len: usize = p.refs.iter().map(|r| format!("  #{r}").chars().count()).sum();
        let text_len = label.chars().count() - refs_len;
        for (i, ch) in label.chars().enumerate() {
            let style = if i >= text_len { Style::default().fg(t.info).bg(fill) } else { st };
            put(buf, p.x + 3 + i as u16, p.y, &ch.to_string(), style);
        }
        put(buf, p.x + p.w - 2, p.y, " ", Style::default().bg(fill));
        put(buf, p.x + p.w - 1, p.y, "▌", Style::default().fg(fill).bg(bg));
    }
    for (i, (a, b, label, _)) in l.edges.iter().filter(|e| e.3).enumerate() {
        let text = format!("⇢ {} → {}  {}", map.nodes[*a].id, map.nodes[*b].id, label.as_deref().unwrap_or(""));
        for (j, ch) in text.chars().enumerate() {
            let fg = if j == 0 { t.agent } else { t.text_muted };
            put(buf, j as u16, l.links_y + i as u16, &ch.to_string(), Style::default().fg(fg).bg(bg));
        }
    }
}

fn flow_text(buf: &mut Buffer, l: &Layout, t: &Theme, bg: Color, put: &mut Put) {
    let edge = Style::default().fg(t.text_muted).bg(bg);
    for (a, b, label, _) in &l.edges {
        let (x0, y0, mid, x1, y1) = route(l, *a, *b);
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
            // The label as a pill sitting on the arrow.
            let pill = t.surface_raised;
            let lx = mid + 1;
            put(buf, lx, y1, "▐", Style::default().fg(pill).bg(bg));
            for (i, ch) in lb.chars().enumerate() {
                put(buf, lx + 1 + i as u16, y1, &ch.to_string(), Style::default().fg(t.text).bg(pill));
            }
            put(buf, lx + 1 + lb.chars().count() as u16, y1, "▌", Style::default().fg(pill).bg(bg));
        }
    }
    for p in &l.nodes {
        flow_node(buf, p, t, bg, put);
    }
}

/// One flowchart symbol as a filled card. Edges are half blocks in the fill colour so the fill
/// starts at the edge; each symbol gets its own ends (round, pointed, slanted, folded).
fn flow_node(buf: &mut Buffer, p: &Placed, t: &Theme, bg: Color, put: &mut Put) {
    let (fill, line) = flow_colours(p, t, bg);
    let (r, b) = (p.x + p.w - 1, p.y + p.h - 1);
    let mids = p.h.saturating_sub(2);
    // Whether row y lies in the upper half of the symbol's inner rows.
    let upper = |y: u16| (y - p.y - 1) * 2 < mids;
    let on = |s: &'static str| (s, Style::default().fg(fill).bg(bg));
    let none = ("", Style::default());
    let shape = p.shape;
    for y in p.y..=b {
        for x in p.x..=r {
            let (left, right, top, bottom) = (x == p.x, x == r, y == p.y, y == b);
            let corner = (left || right) && (top || bottom);
            let (sym, st): (&str, Style) = if !(left || right || top || bottom) {
                (" ", Style::default().bg(fill))
            } else {
                match shape {
                    // Rounded ends: notched quadrant caps.
                    NodeShape::Start | NodeShape::End | NodeShape::Connector => {
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
                    NodeShape::Decision | NodeShape::Prep => {
                        let taper = shape == NodeShape::Decision && (x <= p.x + 2 || x + 2 >= r);
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
                    NodeShape::Io => match (left, right, top, bottom) {
                        (true, _, true, _) | (_, true, _, true) => none,
                        (true, _, _, true) => on("▀"),
                        (_, true, true, _) => on("▄"),
                        (true, _, _, _) => on("◢"),
                        (_, true, _, _) => on("◤"),
                        (_, _, true, _) => on("▄"),
                        _ => on("▀"),
                    },
                    // Flat on the left, round on the right.
                    NodeShape::Delay => match (left, right, top, bottom) {
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
                        (true, _, true, _) => on(if shape == NodeShape::Manual { "▁" } else { "▗" }),
                        (_, true, true, _) => if shape == NodeShape::Note { none } else { on("▖") },
                        (true, _, _, true) => on("▝"),
                        (_, true, _, true) => on("▘"),
                        // Manual input: the top edge slopes up to the right.
                        (_, _, true, _) if shape == NodeShape::Manual && x < p.x + p.w / 2 => on("▁"),
                        (_, _, true, _) if shape == NodeShape::Note && x == r - 1 => on("▖"),
                        (_, _, true, _) => on("▄"),
                        // A document's wavy bottom.
                        (_, _, _, true) if shape == NodeShape::Document && (x - p.x) % 4 >= 2 => on("▔"),
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
    match shape {
        NodeShape::Subprocess => {
            for y in p.y + 1..b {
                put(buf, p.x + 1, y, "│", mark);
                put(buf, r - 1, y, "│", mark);
            }
        }
        NodeShape::Data => {
            for x in p.x + 1..r {
                put(buf, x, p.y + 1, "‿", mark);
            }
        }
        NodeShape::Note => put(buf, r - 1, p.y + 1, "◥", Style::default().fg(bg).bg(fill)),
        _ => {}
    }
    let first_row = p.y + 1 + u16::from(shape == NodeShape::Data);
    for (i, ln) in p.lines.iter().enumerate() {
        let y = first_row + i as u16;
        let tx = p.x + (p.w - ln.chars().count() as u16) / 2;
        let style = match (shape, i) {
            (NodeShape::Connector, _) | (NodeShape::Gate(_), 1) => Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            (NodeShape::Gate(_), 0) => Style::default().fg(line),
            (NodeShape::Note, _) => Style::default().fg(t.text_muted).add_modifier(Modifier::ITALIC),
            (_, 0) => Style::default().fg(t.text_muted),
            _ => Style::default().fg(t.text).add_modifier(Modifier::BOLD),
        };
        for (j, ch) in ln.chars().enumerate() {
            put(buf, tx + j as u16, y, &ch.to_string(), style.bg(fill));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FlowEdge, FlowNode};

    fn node(id: &str, text: &str, shape: NodeShape) -> FlowNode {
        FlowNode { id: id.into(), text: text.into(), shape, changed: false, status: NodeStatus::Open, refs: vec![] }
    }

    fn edge(a: &str, b: &str) -> FlowEdge {
        FlowEdge { from: a.into(), to: b.into(), label: None, link: false }
    }

    fn flow() -> FlowMap {
        FlowMap {
            kind: MapKind::Flowchart,
            nodes: vec![node("n1", "Start", NodeShape::Start), node("n2", "OK?", NodeShape::Decision), node("n3", "A", NodeShape::Process), node("n4", "B", NodeShape::Process)],
            edges: vec![edge("n1", "n2"), edge("n2", "n3"), edge("n2", "n4")],
        }
    }

    #[test]
    fn ranks_go_left_to_right_and_siblings_stack() {
        let l = layout(&flow());
        assert!(l.nodes[0].x < l.nodes[1].x && l.nodes[1].x < l.nodes[2].x);
        assert_eq!(l.nodes[2].x, l.nodes[3].x);
        assert!(l.nodes[2].y + l.nodes[2].h <= l.nodes[3].y);
    }

    #[test]
    fn a_mind_map_is_a_tree_with_the_parent_centred_on_its_children() {
        let mut m = FlowMap {
            kind: MapKind::Mindmap,
            nodes: vec![node("n1", "root", NodeShape::Topic), node("n2", "a", NodeShape::Fact), node("n3", "b", NodeShape::Question), node("n4", "c", NodeShape::Idea)],
            edges: vec![edge("n1", "n2"), edge("n1", "n3"), edge("n1", "n4")],
        };
        m.edges.push(FlowEdge { from: "n2".into(), to: "n4".into(), label: Some("supports".into()), link: true });
        let l = layout(&m);
        assert!(l.nodes[1].x > l.nodes[0].x);
        assert_eq!(l.nodes[0].y, (l.nodes[1].y + l.nodes[3].y) / 2);
        assert_eq!(l.height, l.links_y + 1, "one row for the cross link");
    }

    #[test]
    fn every_svg_parses() {
        let t = Theme::builtin("dark").unwrap();
        for m in [flow(), crate::debug::sample_flow_gallery(), crate::debug::sample_mind_gallery(), crate::debug::sample_map()] {
            let s = svg(&m, &t, 140, 40);
            assert!(resvg::usvg::Tree::from_str(&s, &resvg::usvg::Options::default()).is_ok(), "{s}");
        }
    }
}
