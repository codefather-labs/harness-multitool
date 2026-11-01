//! Mind maps (ADR 0011: a map of what is known about one topic): a tree from the root, laid
//! out left to right with the parent centred on its children, drawn as kind-coloured pills
//! joined by branches, and the cross links listed under the tree.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::maps::{CH, CW, Put, clip, esc, fit, hex, mix, origin, svg_open};
use crate::model::{MapElement, MindKind, MindMap, NodeStatus};
use crate::theme::Theme;

/// Rows between siblings.
const VGAP: u16 = 1;
/// Widest node text before an ellipsis.
const TEXT_MAX: usize = 34;

#[derive(Debug, Clone)]
struct Placed {
    x: u16,
    y: u16,
    w: u16,
    label: String,
    kind: MindKind,
    status: NodeStatus,
    refs_len: usize,
    changed: bool,
}

#[derive(Debug, Clone)]
pub struct Layout {
    nodes: Vec<Placed>,
    children: Vec<Vec<usize>>,
    /// (from, to, label) by node index.
    links: Vec<(usize, usize, String)>,
    links_y: u16,
    pub width: u16,
    pub height: u16,
}

/// A kind's glyph and colour.
fn style(kind: MindKind, status: NodeStatus, t: &Theme) -> (&'static str, Color) {
    match kind {
        MindKind::Topic => ("◆", t.accent),
        MindKind::Goal => ("◎", t.accent),
        MindKind::Decision => ("◈", t.agent),
        MindKind::Fact => ("●", t.success),
        MindKind::Hypothesis => ("◐", t.warning),
        MindKind::Question => ("?", t.info),
        MindKind::Task if status == NodeStatus::Done => ("☑", t.success),
        MindKind::Task => ("☐", t.text),
        MindKind::Idea => ("✦", t.warning),
        MindKind::DeadEnd => ("✕", t.error),
        MindKind::Note => ("✎", t.text_muted),
    }
}

/// A node's fill and line colour.
fn colours(p: &Placed, t: &Theme, bg: Color) -> (Color, Color) {
    let (_, c) = style(p.kind, p.status, t);
    if p.changed {
        return (mix(t.accent_line, bg, 0.30), t.accent_line);
    }
    match p.kind {
        MindKind::Topic => (mix(c, bg, 0.32), c),
        MindKind::Note | MindKind::Task => (t.surface_raised, t.border_subtle),
        _ => (mix(c, bg, 0.16), c),
    }
}

pub fn layout(map: &MindMap) -> Layout {
    let n = map.nodes.len();
    let index = |id: &str| map.nodes.iter().position(|x| x.id == id);
    let mut nodes: Vec<Placed> = map
        .nodes
        .iter()
        .map(|nd| {
            // Journal refs, then linked flowcharts.
            let refs: String = nd.refs.iter().map(|r| format!("  #{r}")).chain(nd.flows.iter().map(|f| format!("  ▸ flow #{f}"))).collect();
            let label = format!("{}{}", fit(&nd.text, TEXT_MAX), refs);
            // Pill: cap, glyph, space, label, space, cap.
            let w = label.chars().count() as u16 + 5;
            Placed { x: 0, y: 0, w, label, kind: nd.kind, status: nd.status, refs_len: refs.chars().count(), changed: nd.changed }
        })
        .collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut has_parent = vec![false; n];
    for br in &map.branches {
        if let (Some(a), Some(b)) = (index(&br.from), index(&br.to))
            && !has_parent[b]
            && a != b
        {
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
    // Columns by depth: as wide as the widest node one level up, then room for the branches.
    let max_d = depth.iter().copied().max().unwrap_or(0);
    let mut col_x = vec![0u16; max_d + 1];
    for d in 1..=max_d {
        let w = (0..n).filter(|&i| depth[i] == d - 1).map(|i| nodes[i].w).max().unwrap_or(0);
        col_x[d] = col_x[d - 1] + w + 6;
    }
    // Rows: leaves one after another, a parent centred on its children.
    fn place(i: usize, children: &[Vec<usize>], nodes: &mut [Placed], next: &mut u16) -> u16 {
        if children[i].is_empty() {
            nodes[i].y = *next;
            *next += 1 + VGAP;
            return nodes[i].y;
        }
        let ys: Vec<u16> = children[i].clone().into_iter().map(|c| place(c, children, nodes, next)).collect();
        nodes[i].y = (ys[0] + ys[ys.len() - 1]) / 2;
        nodes[i].y
    }
    let mut next = 0u16;
    for &r in &roots {
        place(r, &children, &mut nodes, &mut next);
    }
    for i in 0..n {
        nodes[i].x = col_x[depth[i]];
    }
    // Listed under the tree: cross links, and every branch or link that points at flowcharts.
    let flows = |f: &[u32]| f.iter().map(|x| format!("  ▸ flow #{x}")).collect::<String>();
    let mut links: Vec<(usize, usize, String)> =
        map.links.iter().filter_map(|l| Some((index(&l.from)?, index(&l.to)?, format!("{}{}", l.label, flows(&l.flows))))).collect();
    links.extend(
        map.branches
            .iter()
            .filter(|b| !b.flows.is_empty())
            .filter_map(|b| Some((index(&b.from)?, index(&b.to)?, format!("branch{}", flows(&b.flows))))),
    );
    let tree_h = next.saturating_sub(VGAP);
    let width = nodes.iter().map(|p| p.x + p.w).max().unwrap_or(0).max(if links.is_empty() { 0 } else { 40 });
    let links_y = tree_h + u16::from(!links.is_empty());
    let height = links_y + links.len() as u16;
    Layout { nodes, children, links, links_y, width, height }
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

/// The map in text, centred in `area`. Returns its clickable parts in buffer cells: each node
/// with journal refs, and each `#N` ref chip on it (drawn later, so a chip wins over its node).
pub fn draw_text(buf: &mut Buffer, area: Rect, map: &MindMap, t: &Theme, bg: Color, pan: (u16, u16)) -> Vec<(Rect, MapElement)> {
    let l = layout(map);
    // Centred when the map fits, else drawn from the panned position (cut at the area).
    let ox = origin(area.x, area.width, l.width, pan.0);
    let oy = origin(area.y, area.height, l.height, pan.1);
    let mut hits = Vec::new();
    for (i, (p, nd)) in l.nodes.iter().zip(&map.nodes).enumerate() {
        if nd.refs.is_empty() {
            continue;
        }
        if let Some(r) = clip(area, ox + p.x as i32, oy + p.y as i32, p.w, 1) {
            hits.push((r, MapElement::Node(i)));
        }
        // The chips follow the text: "  #137" each.
        let mut at = (p.label.chars().count() - p.refs_len) as u16;
        for r in &nd.refs {
            let w = format!("#{r}").chars().count() as u16;
            if let Some(rect) = clip(area, ox + (p.x + 3 + at + 2) as i32, oy + p.y as i32, w, 1) {
                hits.push((rect, MapElement::Entry(*r)));
            }
            at += 2 + w;
        }
    }
    let mut put = |buf: &mut Buffer, x: u16, y: u16, s: &str, st: Style| {
        let (px, py) = (ox + x as i32, oy + y as i32);
        if px >= 0 && py >= 0 && area.contains((px as u16, py as u16).into()) {
            buf[(px as u16, py as u16)].set_symbol(s).set_style(st);
        }
    };
    text(buf, &l, map, t, bg, &mut put);
    hits
}

fn text(buf: &mut Buffer, l: &Layout, map: &MindMap, t: &Theme, bg: Color, put: &mut Put) {
    let edge = Style::default().fg(t.border_subtle).bg(bg);
    // Branches: parent -> spine -> children, joined with box-drawing characters.
    for (a, ks) in l.children.iter().enumerate() {
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
        let (fill, _) = colours(p, t, bg);
        let (glyph, gc) = style(p.kind, p.status, t);
        put(buf, p.x, p.y, "▐", Style::default().fg(fill).bg(bg));
        put(buf, p.x + 1, p.y, glyph, Style::default().fg(gc).bg(fill).add_modifier(Modifier::BOLD));
        put(buf, p.x + 2, p.y, " ", Style::default().bg(fill));
        let mut st = Style::default().fg(t.text).bg(fill);
        if p.kind == MindKind::Topic {
            st = st.add_modifier(Modifier::BOLD);
        }
        match p.status {
            NodeStatus::Done => st = st.fg(t.text_muted),
            NodeStatus::Dropped => st = st.fg(t.text_muted).add_modifier(Modifier::CROSSED_OUT),
            NodeStatus::Open => {}
        }
        let text_len = p.label.chars().count() - p.refs_len;
        for (i, ch) in p.label.chars().enumerate() {
            let style = if i >= text_len { Style::default().fg(t.info).bg(fill) } else { st };
            put(buf, p.x + 3 + i as u16, p.y, &ch.to_string(), style);
        }
        put(buf, p.x + p.w - 2, p.y, " ", Style::default().bg(fill));
        put(buf, p.x + p.w - 1, p.y, "▌", Style::default().fg(fill).bg(bg));
    }
    for (i, (a, b, label)) in l.links.iter().enumerate() {
        let line = format!("⇢ {} → {}  {label}", map.nodes[*a].id, map.nodes[*b].id);
        for (j, ch) in line.chars().enumerate() {
            let fg = if j == 0 { t.agent } else { t.text_muted };
            put(buf, j as u16, l.links_y + i as u16, &ch.to_string(), Style::default().fg(fg).bg(bg));
        }
    }
}

/// The map as SVG in the theme's colours, `cols` x `rows` cells big, the layout centred.
pub fn svg(map: &MindMap, t: &Theme, cols: u16, rows: u16, pan: (u16, u16)) -> String {
    let l = layout(map);
    let bg = t.background;
    let ox = origin(0, cols, l.width, pan.0);
    let oy = origin(0, rows, l.height, pan.1);
    let px = |x: u16| (x as i32 + ox) as f32 * CW;
    let py = |y: u16| (y as i32 + oy) as f32 * CH;
    let (line, text, muted) = (hex(t.border_subtle), hex(t.text), hex(t.text_muted));
    let mut s = svg_open(cols, rows, t.text_muted);
    // Branches: a curve from the parent's right end to the child's left end.
    for (a, ks) in l.children.iter().enumerate() {
        for &b in ks {
            let (pa, pb) = (&l.nodes[a], &l.nodes[b]);
            let (x0, y0) = (px(pa.x + pa.w) - 2.0, py(pa.y) + CH / 2.0);
            let (x1, y1) = (px(pb.x) + 2.0, py(pb.y) + CH / 2.0);
            let m = (x0 + x1) / 2.0;
            s += &format!(r#"<path d="M{x0} {y0} C{m} {y0} {m} {y1} {x1} {y1}" fill="none" stroke="{line}" stroke-width="2"/>"#);
        }
    }
    for p in &l.nodes {
        let (fill, stroke) = colours(p, t, bg);
        let (glyph, gc) = style(p.kind, p.status, t);
        let (x, y, w) = (px(p.x), py(p.y) + 2.0, p.w as f32 * CW);
        let sw = if p.kind == MindKind::Topic { 2.5 } else { 1.5 };
        s += &format!(r#"<rect x="{x}" y="{y}" width="{w}" height="{}" rx="11" fill="{}" stroke="{}" stroke-width="{sw}"/>"#, CH - 4.0, hex(fill), hex(stroke));
        let dropped = p.status == NodeStatus::Dropped;
        let tc = if dropped || p.status == NodeStatus::Done { muted.clone() } else { text.clone() };
        let deco = if dropped { r#" text-decoration="line-through""# } else { "" };
        let weight = if p.kind == MindKind::Topic { r#" font-weight="bold""# } else { "" };
        s += &format!(r#"<text x="{}" y="{}" fill="{}">{}</text>"#, x + CW, y + 17.0, hex(gc), esc(glyph));
        s += &format!(r#"<text x="{}" y="{}" fill="{tc}"{deco}{weight}>{}</text>"#, x + CW * 3.0, y + 17.0, esc(&p.label));
    }
    // Cross links: out of one node's right end into the other's, bowing out to the right so
    // they never cross node labels; listed under the tree with their labels.
    for (i, (a, b, label)) in l.links.iter().enumerate() {
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
            esc(label)
        );
    }
    s += "</svg>";
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Branch, MindLink, MindNode};

    fn node(id: &str, text: &str, kind: MindKind) -> MindNode {
        MindNode { id: id.into(), text: text.into(), kind, status: NodeStatus::Open, refs: vec![], flows: vec![], changed: false }
    }

    #[test]
    fn a_mind_map_is_a_tree_with_the_parent_centred_on_its_children() {
        let m = MindMap {
            nodes: vec![node("n1", "root", MindKind::Topic), node("n2", "a", MindKind::Fact), node("n3", "b", MindKind::Question), node("n4", "c", MindKind::Idea)],
            branches: ["n2", "n3", "n4"].iter().map(|c| Branch { from: "n1".into(), to: c.to_string(), flows: vec![] }).collect(),
            links: vec![MindLink { from: "n2".into(), to: "n4".into(), label: "supports".into(), flows: vec![] }],
        };
        let l = layout(&m);
        assert!(l.nodes[1].x > l.nodes[0].x);
        assert_eq!(l.nodes[0].y, (l.nodes[1].y + l.nodes[3].y) / 2);
        assert_eq!(l.height, l.links_y + 1, "one row for the cross link");
    }

    #[test]
    fn flowchart_links_show_on_nodes_and_under_the_tree() {
        let mut m = crate::debug::sample_mind_gallery();
        m.nodes[0].flows = vec![4];
        m.branches[0].flows = vec![7];
        let l = layout(&m);
        assert!(l.nodes[0].label.contains("▸ flow #4"));
        assert!(l.links.iter().any(|(_, _, s)| s.contains("▸ flow #7")));
    }

    #[test]
    fn the_gallery_svg_parses() {
        let t = Theme::builtin("dark").unwrap();
        let s = svg(&crate::debug::sample_mind_gallery(), &t, 140, 30, (0, 0));
        assert!(resvg::usvg::Tree::from_str(&s, &resvg::usvg::Options::default()).is_ok());
    }
}
