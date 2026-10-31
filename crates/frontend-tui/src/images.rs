//! Graphics drawn as pictures rather than cells: the logo, icons and large text. `view::draw`
//! reserves their cells, writes a text fallback into them and returns a `Placement` for each;
//! the frontend shows the picture there through the terminal's graphics protocol, the snapshot
//! tool pastes it into the PNG. Both get the pixels from `rasterize`, from vector sources
//! (journal: vector rule; operator decision: icons and big text as pictures).

use fontdue::{Font, FontSettings};
use image::{Rgba, RgbaImage};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use resvg::{tiny_skia, usvg};

use crate::theme::Theme;

const LOGO_SVG: &str = include_str!("../../../docs/design/logo/multitool.svg");
const BOLD: &[u8] = include_bytes!("../fonts/DejaVuSansMono-Bold.ttf");

/// The icon set (docs/design/icons): 24x24 strokes in `#000000`, recoloured per use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    Clock,
    Bolt,
    File,
    FilePlus,
    Branch,
    Play,
    Gear,
    Map,
    Help,
    Info,
}

impl Icon {
    fn svg(self) -> &'static str {
        match self {
            Icon::Clock => include_str!("../../../docs/design/icons/clock.svg"),
            Icon::Bolt => include_str!("../../../docs/design/icons/bolt.svg"),
            Icon::File => include_str!("../../../docs/design/icons/file.svg"),
            Icon::FilePlus => include_str!("../../../docs/design/icons/file-plus.svg"),
            Icon::Branch => include_str!("../../../docs/design/icons/branch.svg"),
            Icon::Play => include_str!("../../../docs/design/icons/play.svg"),
            Icon::Gear => include_str!("../../../docs/design/icons/gear.svg"),
            Icon::Map => include_str!("../../../docs/design/icons/map.svg"),
            Icon::Help => include_str!("../../../docs/design/icons/help.svg"),
            Icon::Info => include_str!("../../../docs/design/icons/info.svg"),
        }
    }

    /// The character a terminal without graphics shows instead.
    pub fn fallback(self) -> &'static str {
        match self {
            Icon::Clock => "◷",
            Icon::Bolt => "↯",
            Icon::File => "▤",
            Icon::FilePlus => "+",
            Icon::Branch => "⑂",
            Icon::Play => "▶",
            Icon::Gear => "⚙",
            Icon::Map => "▦",
            Icon::Help => "?",
            Icon::Info => "i",
        }
    }
}

/// Which theme colour a picture is drawn in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tint {
    Text,
    Muted,
    Accent,
    Info,
}

impl Tint {
    pub fn color(self, t: &Theme) -> Color {
        match self {
            Tint::Text => t.text,
            Tint::Muted => t.text_muted,
            Tint::Accent => t.accent,
            Tint::Info => t.info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Graphic {
    /// The multitool logo.
    Logo,
    Icon(Icon, Tint),
    /// A line of bold text, as large as its cells allow.
    Text(String, Tint),
}

/// Where a graphic goes, in cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub rect: Rect,
    pub graphic: Graphic,
}

fn hex(c: Color) -> String {
    let [r, g, b] = rgb(c);
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn rgb(c: Color) -> [u8; 3] {
    match c {
        Color::Rgb(r, g, b) => [r, g, b],
        _ => [255, 0, 255],
    }
}

/// The logo coloured for the theme: tools in the text colour, the handle in the brand orange,
/// outline and rivets in a dark ink (text on light themes, background on dark ones).
fn logo_svg(t: &Theme) -> String {
    let ink = if t.dark { t.background } else { t.text };
    LOGO_SVG.replace("#20242A", &hex(t.text)).replace("#1A1D22", &hex(ink)).replace("#F5A623", &hex(t.brand))
}

fn render_svg(src: &str, width: u32, height: u32, inset: f32) -> RgbaImage {
    let tree = usvg::Tree::from_str(src, &usvg::Options::default()).expect("embedded SVG parses");
    let size = tree.size();
    let (w, h) = (width as f32 * inset, height as f32 * inset);
    let scale = (w / size.width()).min(h / size.height());
    let dx = (width as f32 - size.width() * scale) / 2.0;
    let dy = (height as f32 - size.height() * scale) / 2.0;
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1)).expect("non-zero pixmap");
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    // tiny-skia stores premultiplied RGBA; un-premultiply for `image`.
    let mut out = RgbaImage::new(pixmap.width(), pixmap.height());
    for (px, c) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = c.demultiply();
        *px = Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// Bold text, letters about 80% of the height, centred; shrunk to fit the width.
fn render_text(text: &str, color: Color, width: u32, height: u32) -> RgbaImage {
    let font = Font::from_bytes(BOLD, FontSettings::default()).expect("embedded font parses");
    let advance = |px: f32| text.chars().map(|c| font.metrics(c, px).advance_width).sum::<f32>();
    let mut px = height as f32 * 0.82;
    if advance(px) > width as f32 {
        px *= width as f32 / advance(px);
    }
    let (ascent, descent) = font
        .horizontal_line_metrics(px)
        .map(|m| (m.ascent, -m.descent))
        .unwrap_or((px * 0.8, px * 0.2));
    let baseline = ((height as f32 - (ascent + descent)) / 2.0 + ascent).round() as i32;
    let mut x = (width as f32 - advance(px)) / 2.0;
    let [r, g, b] = rgb(color);
    let mut out = RgbaImage::new(width.max(1), height.max(1));
    for c in text.chars() {
        let (m, bitmap) = font.rasterize(c, px);
        let gx = x.round() as i32 + m.xmin;
        let gy = baseline - m.height as i32 - m.ymin;
        for (i, a) in bitmap.iter().enumerate() {
            let (qx, qy) = (gx + (i % m.width.max(1)) as i32, gy + (i / m.width.max(1)) as i32);
            if *a > 0 && qx >= 0 && qy >= 0 && (qx as u32) < out.width() && (qy as u32) < out.height() {
                out.put_pixel(qx as u32, qy as u32, Rgba([r, g, b, *a]));
            }
        }
        x += m.advance_width;
    }
    out
}

/// Renders the graphic to exactly `width` x `height` pixels, transparent around it.
pub fn rasterize(g: &Graphic, t: &Theme, width: u32, height: u32) -> RgbaImage {
    match g {
        Graphic::Logo => render_svg(&logo_svg(t), width, height, 1.0),
        // Icons keep a margin so they sit inside their cells like a glyph.
        Graphic::Icon(i, tint) => render_svg(&i.svg().replace("#000000", &hex(tint.color(t))), width, height, 0.78),
        Graphic::Text(s, tint) => render_text(s, tint.color(t), width, height),
    }
}

/// The logo drawn by hand as pixel art for terminals without graphics: 52 x 8 pixels, two per
/// cell (half blocks), so 52 x 4 cells. T tool, O handle, H highlight, X rivet hole, . empty.
pub const LOGO_ART: [&str; 8] = [
    "TTTTTTTTTTTT...........................T............",
    "........TTTT..OOOOOOOOOOOOOOOOOOOOOOO..T...T..T..T..",
    "TTTTTTTTTTTT.OOOOHHHHHHHHHHHHHHHOOOOOO.TT.TT.TT.TT..",
    "........TTTTTOOOOOOOOOOOOOOOOOOOOOOOOOTTTTTTTTTTTTT.",
    "TTTTTTTTTTTTTOOOXXOOOOOOOOOOOOOOOXXOOOTTTTTTTTTTTTTT",
    "........TTTT.OOOXXOOOOOOOOOOOOOOOXXOOOTTTTTTTTTTTTT.",
    "TTTTTTTTTTTT.OOOOOOOOOOOOOOOOOOOOOOOOOTTTTTTTTTTTT..",
    "..............OOOOOOOOOOOOOOOOOOOOOOO...............",
];

/// Draws `LOGO_ART` at `(x, y)` with half blocks in the theme's logo colours.
fn logo_art(buf: &mut Buffer, x: u16, y: u16, t: &Theme, bg: Color) {
    let ink = if t.dark { t.background } else { t.text };
    let colour = |c: u8| -> Option<Color> {
        match c {
            b'T' => Some(t.text),
            b'O' => Some(t.brand),
            b'H' => Some(Color::Rgb(0xFF, 0xE3, 0xB0)),
            b'X' => Some(ink),
            _ => None,
        }
    };
    for row in 0..LOGO_ART.len() / 2 {
        let (up, down) = (LOGO_ART[row * 2].as_bytes(), LOGO_ART[row * 2 + 1].as_bytes());
        for col in 0..up.len() {
            let (a, b) = (colour(up[col]), colour(down[col]));
            let (sym, fg, cell_bg) = match (a, b) {
                (None, None) => (" ", bg, bg),
                (Some(a), None) => ("▀", a, bg),
                (None, Some(b)) => ("▄", b, bg),
                (Some(a), Some(b)) if a == b => ("█", a, bg),
                (Some(a), Some(b)) => ("▀", a, b),
            };
            let pos = (x + col as u16, y + row as u16);
            if buf.area.contains(pos.into()) {
                buf[pos].set_symbol(sym).set_fg(fg).set_bg(cell_bg);
            }
        }
    }
}

/// The text fallback written into a placement's cells, for terminals without graphics: an
/// icon becomes its glyph, text becomes bold text on the middle row, the logo becomes its text
/// mark (or, where 52 x 4 cells are free, the hand-drawn LOGO_ART). Never pixel art computed
/// from a picture: that showed seams and turned text into noise (operator's GNOME Terminal
/// screenshots, 2026-10-31).
pub fn fallback(buf: &mut Buffer, p: &Placement, t: &Theme) {
    let r = p.rect.intersection(buf.area);
    if r.is_empty() {
        return;
    }
    let mid = r.y + r.height.saturating_sub(1) / 2;
    match &p.graphic {
        Graphic::Icon(i, tint) => {
            let y = r.y + (r.height.saturating_sub(1)) / 2;
            let x = r.x + r.width.saturating_sub(1) / 2;
            buf[(x, y)].set_symbol(i.fallback()).set_fg(tint.color(t));
        }
        Graphic::Text(s, tint) => {
            let x = r.x + r.width.saturating_sub(s.chars().count() as u16) / 2;
            let style = ratatui::style::Style::default().fg(tint.color(t)).add_modifier(ratatui::style::Modifier::BOLD);
            buf.set_stringn(x, mid, s, r.width as usize, style);
        }
        Graphic::Logo if r.width >= LOGO_ART[0].len() as u16 && r.height >= 4 => {
            let bg = match buf[(r.x, r.y)].bg {
                Color::Reset => t.background,
                c => c,
            };
            let w = LOGO_ART[0].len() as u16;
            logo_art(buf, r.x + (r.width - w) / 2, r.y + (r.height - 4) / 2, t, bg);
        }
        Graphic::Logo => {
            let line = ratatui::text::Line::from(crate::widgets::logo_mark(t));
            let w = line.width() as u16;
            buf.set_line(r.x + r.width.saturating_sub(w) / 2, mid, &line, r.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_rasterizes_with_the_brand_handle() {
        let t = Theme::builtin("dark").unwrap();
        let img = rasterize(&Graphic::Logo, &t, 320, 88);
        let brand = img.pixels().filter(|p| p[3] == 255 && p[0] == 0xF5 && p[1] == 0xA6 && p[2] == 0x23).count();
        assert!(brand > 500, "brand-orange handle pixels: {brand}");
    }

    #[test]
    fn every_icon_rasterizes() {
        let t = Theme::builtin("light").unwrap();
        for i in [Icon::Clock, Icon::Bolt, Icon::File, Icon::FilePlus, Icon::Branch, Icon::Play, Icon::Gear, Icon::Map, Icon::Help, Icon::Info] {
            let img = rasterize(&Graphic::Icon(i, Tint::Text), &t, 24, 26);
            assert!(img.pixels().any(|p| p[3] > 200), "{i:?} drew nothing");
        }
    }

    #[test]
    fn big_text_rasterizes() {
        let t = Theme::builtin("dark").unwrap();
        let img = rasterize(&Graphic::Text("harness-multitool".into(), Tint::Text), &t, 480, 52);
        assert!(img.pixels().filter(|p| p[3] > 200).count() > 2000);
    }
}
