//! What the two map renderers share (`mindmap`, `flowchart`; ADR 0011): the SVG cell size,
//! text wrapping, colours. Both lay a map out on the terminal's cell grid once and draw that
//! layout twice: as block and box-drawing characters (the text form, the main one:
//! journal#227) and as SVG for graphics terminals (journal#140).

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Style};

/// Pixels per cell in the SVG's coordinates; the picture is scaled to the real cell size.
pub const CW: f32 = 12.0;
pub const CH: f32 = 26.0;

/// Draws one cell of a layout, relative to the layout's origin, clipped to the map's area.
pub type Put<'a> = dyn FnMut(&mut Buffer, u16, u16, &str, Style) + 'a;

pub fn wrap(text: &str, max: usize) -> Vec<String> {
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

pub fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

pub fn hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => "#888888".into(),
    }
}

pub fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `a` laid over `b` at `alpha`.
pub fn mix(a: Color, b: Color, alpha: f32) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f32 * alpha + y as f32 * (1.0 - alpha)).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => b,
    }
}

/// The SVG prologue for a map `cols` x `rows` cells big, with an arrowhead marker.
pub fn svg_open(cols: u16, rows: u16, arrow: Color) -> String {
    let (w, h) = (cols as f32 * CW, rows as f32 * CH);
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" font-family="DejaVu Sans Mono, monospace" font-size="17"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto"><path d="M0 0 L10 5 L0 10 z" fill="{}"/></marker></defs>"#,
        hex(arrow)
    )
}

/// Where a map `content` cells long starts in an area `len` cells long that starts at `at`:
/// centred when it fits, else from `pan` cells into the map (clamped to its far edge).
pub fn origin(at: u16, len: u16, content: u16, pan: u16) -> i32 {
    if content > len { at as i32 - pan.min(content - len) as i32 } else { (at + (len - content) / 2) as i32 }
}

/// The farthest a map can be panned in an area: (columns, rows) hidden.
pub fn pan_limit(area: (u16, u16), content: (u16, u16)) -> (u16, u16) {
    (content.0.saturating_sub(area.0), content.1.saturating_sub(area.1))
}

/// A rect at a signed position, cut to `area`; None when nothing of it is inside.
pub fn clip(area: ratatui::layout::Rect, x: i32, y: i32, w: u16, h: u16) -> Option<ratatui::layout::Rect> {
    let (x0, y0) = (x.max(area.x as i32), y.max(area.y as i32));
    let (x1, y1) = ((x + w as i32).min((area.x + area.width) as i32), (y + h as i32).min((area.y + area.height) as i32));
    (x0 < x1 && y0 < y1).then(|| ratatui::layout::Rect::new(x0 as u16, y0 as u16, (x1 - x0) as u16, (y1 - y0) as u16))
}
