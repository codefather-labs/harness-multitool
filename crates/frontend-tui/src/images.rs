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
/// The logo's eyes shut: a lid line where each pupil was.
const EYES_CLOSED: &str = r##"<rect x="143" y="53" width="24" height="5" rx="2.5" fill="#1A1D22"/>
  <rect x="348" y="53" width="24" height="5" rx="2.5" fill="#1A1D22"/>
"##;
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
    /// The multitool logo, eyes open or shut.
    Logo(Eyes),
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
fn logo_svg(t: &Theme, eyes: Eyes) -> String {
    let ink = if t.dark { t.background } else { t.text };
    let src = match eyes {
        Eyes::Open => LOGO_SVG.to_string(),
        Eyes::Closed => {
            let (a, b) = (LOGO_SVG.find("<!-- Eyes").expect("eyes in logo"), LOGO_SVG.find("</svg>").expect("svg end"));
            format!("{}{}{}", &LOGO_SVG[..a], EYES_CLOSED, &LOGO_SVG[b..])
        }
    };
    src.replace("#20242A", &hex(t.text)).replace("#1A1D22", &hex(ink)).replace("#F5A623", &hex(t.brand))
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
        Graphic::Logo(eyes) => render_svg(&logo_svg(t, *eyes), width, height, 1.0),
        // Icons keep a margin so they sit inside their cells like a glyph.
        Graphic::Icon(i, tint) => render_svg(&i.svg().replace("#000000", &hex(tint.color(t))), width, height, 0.78),
        Graphic::Text(s, tint) => render_text(s, tint.color(t), width, height),
    }
}

/// The logo drawn by hand as pixel art for terminals without graphics: 52 x 8 pixels, two per
/// cell (half blocks), so 52 x 4 cells. T tool, O handle, H highlight, X rivet hole, . empty.
/// Regular on purpose: three equal 2-px tines, a symmetric rounded handle, identical saw teeth.
pub const LOGO_ART: [&str; 8] = [
    "TTTTTTTTTTT...OOOOOOOOOOOOOOOOOOOOOOOOOO............",
    "TTTTTTTTTTT..OOOHHHHHHHHHHHHHHHHHHHHHHOOO...T..T..T.",
    ".........TTTTOOOOOOOOOOOOOOOOOOOOOOOOOOOOTT.TT.TT.TT",
    "TTTTTTTTTTTTTOOOWXOOOOOOOOOOOOOOOOOOWXOOOTTTTTTTTTTT",
    "TTTTTTTTTTTTTOOOXXOOOOOOOOOOOOOOOOOOXXOOOTTTTTTTTTTT",
    ".........TTTTOOOOOOOOOOOOOOOOOOOOOOOOOOOOTTTTTTTTTTT",
    "TTTTTTTTTTT..OOOOOOOOOOOOOOOOOOOOOOOOOOOO..TTTTTTTT.",
    "TTTTTTTTTTT...OOOOOOOOOOOOOOOOOOOOOOOOOO............",
];

/// `LOGO_ART` with the eyes shut: only the lower lid line is left.
pub const LOGO_ART_CLOSED: [&str; 8] = [
    "TTTTTTTTTTT...OOOOOOOOOOOOOOOOOOOOOOOOOO............",
    "TTTTTTTTTTT..OOOHHHHHHHHHHHHHHHHHHHHHHOOO...T..T..T.",
    ".........TTTTOOOOOOOOOOOOOOOOOOOOOOOOOOOOTT.TT.TT.TT",
    "TTTTTTTTTTTTTOOOOOOOOOOOOOOOOOOOOOOOOOOOOTTTTTTTTTTT",
    "TTTTTTTTTTTTTOOOXXOOOOOOOOOOOOOOOOOOXXOOOTTTTTTTTTTT",
    ".........TTTTOOOOOOOOOOOOOOOOOOOOOOOOOOOOTTTTTTTTTTT",
    "TTTTTTTTTTT..OOOOOOOOOOOOOOOOOOOOOOOOOOOO..TTTTTTTT.",
    "TTTTTTTTTTT...OOOOOOOOOOOOOOOOOOOOOOOOOO............",
];

/// The same multitool for the one-row header: 24 x 3 pixels in sextants (2 x 3 per cell), so
/// 12 cells. Every cell holds at most two colours, which is all a cell can show.
pub const LOGO_MARK: [&str; 3] = [
    "TTTT...OOOOOOOO..T.T.T.T",
    "...TTTOOOXOOXOOOTTTTTTTT",
    "TTTT...OOOOOOOO.TTTTTTT.",
];

/// `LOGO_MARK` with the eyes shut: each eye a two-pixel slit.
pub const LOGO_MARK_CLOSED: [&str; 3] = [
    "TTTT...OOOOOOOO..T.T.T.T",
    "...TTTOOXXOOXXOOTTTTTTTT",
    "TTTT...OOOOOOOO.TTTTTTT.",
];

/// Whether the multitool's eyes (the handle's two rivets) are open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Eyes {
    #[default]
    Open,
    Closed,
}

/// The blinks played on launch, as (start, end) in ms since start: a quick double blink.
pub const BLINKS: [(u64, u64); 2] = [(700, 840), (1000, 1140)];

/// The eyes at `ms` after launch, and when they next change (None: they stay open).
pub fn eyes_at(ms: u64) -> (Eyes, Option<u64>) {
    for (a, b) in BLINKS {
        if ms < a {
            return (Eyes::Open, Some(a));
        }
        if ms < b {
            return (Eyes::Closed, Some(b));
        }
    }
    (Eyes::Open, None)
}

fn art_colour(t: &Theme, c: u8) -> Option<Color> {
    match c {
        b'T' => Some(t.text),
        b'O' => Some(t.brand),
        b'H' => Some(Color::Rgb(0xFF, 0xE3, 0xB0)),
        b'W' => Some(Color::Rgb(0xFF, 0xFF, 0xFF)),
        b'X' => Some(if t.dark { t.background } else { t.text }),
        _ => None,
    }
}

/// Draws pixel art with `pw` x `ph` pixels per cell: 1 x 2 (half blocks) or 2 x 3 (sextants).
/// A cell takes the colours of its pixels as fg and bg; empty pixels show `bg`.
fn pixel_art(buf: &mut Buffer, x: u16, y: u16, art: &[&str], (pw, ph): (usize, usize), t: &Theme, bg: Color) {
    let rows = art.len() / ph;
    let cols = art[0].len() / pw;
    for row in 0..rows {
        for col in 0..cols {
            let mut px: Vec<Color> = Vec::with_capacity(pw * ph);
            for dy in 0..ph {
                for dx in 0..pw {
                    px.push(art_colour(t, art[row * ph + dy].as_bytes()[col * pw + dx]).unwrap_or(bg));
                }
            }
            // Background of the cell: the screen's when the cell shows it or is one solid colour
            // (drawn as █), else the first colour, so a cell never needs more than two.
            let solid = px.iter().all(|c| *c == px[0]);
            let back = if px.contains(&bg) || solid { bg } else { px[0] };
            let fore = px.iter().copied().find(|c| *c != back).unwrap_or(back);
            let mask: u32 = px.iter().enumerate().filter(|(_, c)| **c == fore && fore != back).map(|(i, _)| 1 << i).sum();
            let sym = cell_symbol(mask, pw, ph);
            let pos = (x + col as u16, y + row as u16);
            if buf.area.contains(pos.into()) {
                buf[pos].set_symbol(&sym).set_fg(fore).set_bg(back);
            }
        }
    }
}

/// The block glyph whose set pixels are `mask` (bit i = pixel i, row-major).
fn cell_symbol(mask: u32, pw: usize, ph: usize) -> String {
    let full = (1u32 << (pw * ph)) - 1;
    if mask == 0 {
        return " ".into();
    }
    if mask == full {
        return "█".into();
    }
    match (pw, ph) {
        (1, 2) => if mask == 1 { "▀" } else { "▄" }.into(),
        _ => match mask {
            0b010101 => "▌".into(),
            0b101010 => "▐".into(),
            // Unicode 13 sextants U+1FB00..U+1FB3B skip the two halves above.
            m => {
                let n = if m < 21 { m - 1 } else if m < 42 { m - 2 } else { m - 3 };
                char::from_u32(0x1FB00 + n).map(String::from).unwrap_or_else(|| "█".into())
            }
        },
    }
}

/// Draws `LOGO_ART` at `(x, y)`.
fn logo_art(buf: &mut Buffer, x: u16, y: u16, eyes: Eyes, t: &Theme, bg: Color) {
    let art = if eyes == Eyes::Open { &LOGO_ART } else { &LOGO_ART_CLOSED };
    pixel_art(buf, x, y, art, (1, 2), t, bg);
}

/// The text fallback written into a placement's cells, for terminals without graphics: an
/// icon becomes its glyph, text becomes bold text on the middle row, the logo becomes its text
/// LOGO_MARK in sextants (or, where 52 x 4 cells are free, LOGO_ART in half blocks). Never pixel art computed
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
        Graphic::Logo(eyes) if r.width >= LOGO_ART[0].len() as u16 && r.height >= 4 => {
            let bg = match buf[(r.x, r.y)].bg {
                Color::Reset => t.background,
                c => c,
            };
            let w = LOGO_ART[0].len() as u16;
            logo_art(buf, r.x + (r.width - w) / 2, r.y + (r.height - 4) / 2, *eyes, t, bg);
        }
        Graphic::Logo(eyes) => {
            let mark = if *eyes == Eyes::Open { &LOGO_MARK } else { &LOGO_MARK_CLOSED };
            let bg = match buf[(r.x, mid)].bg {
                Color::Reset => t.background,
                c => c,
            };
            let w = (LOGO_MARK[0].len() / 2) as u16;
            pixel_art(buf, r.x + r.width.saturating_sub(w) / 2, mid, mark, (2, 3), t, bg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_logo_blinks_twice_on_launch_then_stays_open() {
        assert_eq!(eyes_at(0), (Eyes::Open, Some(700)));
        assert_eq!(eyes_at(750), (Eyes::Closed, Some(840)));
        assert_eq!(eyes_at(900), (Eyes::Open, Some(1000)));
        assert_eq!(eyes_at(1100), (Eyes::Closed, Some(1140)));
        assert_eq!(eyes_at(5000), (Eyes::Open, None));
    }

    #[test]
    fn sextant_codes_skip_the_halves() {
        assert_eq!(cell_symbol(1, 2, 3), "\u{1FB00}");
        assert_eq!(cell_symbol(0b010101, 2, 3), "▌");
        assert_eq!(cell_symbol(22, 2, 3), "\u{1FB14}");
        assert_eq!(cell_symbol(62, 2, 3), "\u{1FB3B}");
        assert_eq!(cell_symbol(63, 2, 3), "█");
    }

    #[test]
    fn every_logo_art_cell_needs_at_most_two_colours() {
        let t = Theme::builtin("dark").unwrap();
        for (art, pw, ph) in [(&LOGO_ART[..], 1, 2), (&LOGO_ART_CLOSED[..], 1, 2), (&LOGO_MARK[..], 2, 3), (&LOGO_MARK_CLOSED[..], 2, 3)] {
            for row in 0..art.len() / ph {
                for col in 0..art[0].len() / pw {
                    let mut c: Vec<Option<Color>> = Vec::new();
                    for dy in 0..ph {
                        for dx in 0..pw {
                            let v = art_colour(&t, art[row * ph + dy].as_bytes()[col * pw + dx]);
                            if !c.contains(&v) {
                                c.push(v);
                            }
                        }
                    }
                    assert!(c.len() <= 2, "cell {col},{row} has {} colours", c.len());
                }
            }
        }
    }

    #[test]
    fn logo_rasterizes_with_the_brand_handle() {
        let t = Theme::builtin("dark").unwrap();
        let img = rasterize(&Graphic::Logo(Eyes::Open), &t, 320, 88);
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
