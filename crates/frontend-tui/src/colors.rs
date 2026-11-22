//! Colour depth of the terminal (journal#1615). The themes are 24-bit (`Color::Rgb`), written as
//! SGR 38;2;r;g;b. A terminal without 24-bit colour, such as macOS Terminal before macOS 26, reads
//! r, g and b as SGR codes of their own (4 underline, 7 reverse, 8 hidden…), and the window falls
//! apart. There, every colour of a frame becomes the nearest of xterm's 256 before it is written.

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// How many colours the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    Indexed256,
}

/// The terminal's colour depth from its environment: `MULTITOOL_COLORS` (`256` or `truecolor`)
/// when set; else 256 colours in macOS Terminal unless it says `COLORTERM=truecolor`; else 24-bit.
pub fn depth(env: impl Fn(&str) -> Option<String>) -> Depth {
    match env("MULTITOOL_COLORS").as_deref().map(str::trim) {
        Some("256") => return Depth::Indexed256,
        Some("truecolor" | "24bit") => return Depth::TrueColor,
        _ => {}
    }
    let says_truecolor = matches!(env("COLORTERM").as_deref(), Some("truecolor" | "24bit"));
    if env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal") && !says_truecolor {
        return Depth::Indexed256;
    }
    Depth::TrueColor
}

/// The levels of xterm's 6×6×6 colour cube (indexes 16 to 231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn nearest_level(v: u8) -> usize {
    (0..CUBE.len()).min_by_key(|&i| (CUBE[i] as i32 - v as i32).abs()).unwrap_or(0)
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2);
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

/// The xterm-256 index nearest to an RGB colour: the cube's or the grey ramp's (232 to 255).
pub fn nearest_index(r: u8, g: u8, b: u8) -> u8 {
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (CUBE[ri], CUBE[gi], CUBE[bi]);
    let cube_index = 16 + 36 * ri + 6 * gi + bi;
    let mean = (r as u32 + g as u32 + b as u32) / 3;
    let step = ((mean.saturating_sub(8) + 5) / 10).min(23) as u8;
    let grey = 8 + 10 * step;
    if distance((r, g, b), (grey, grey, grey)) < distance((r, g, b), cube) { 232 + step } else { cube_index as u8 }
}

/// A colour as the terminal can show it.
pub fn fit(c: Color, depth: Depth) -> Color {
    match (c, depth) {
        (Color::Rgb(r, g, b), Depth::Indexed256) => Color::Indexed(nearest_index(r, g, b)),
        _ => c,
    }
}

/// Whether the terminal draws block elements from its font, so that rows of them show gaps:
/// macOS Terminal (journal#1619). Others draw them as boxes of their own.
pub fn font_blocks(env: impl Fn(&str) -> Option<String>) -> bool {
    env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
}

static FONT_BLOCKS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Says, for this process, that the terminal draws blocks from its font (journal#1642): what
/// draws half rows, the spacers between cards, then keeps to whole cells.
pub fn set_font_blocks(on: bool) {
    FONT_BLOCKS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether [`set_font_blocks`] said so.
pub fn font_blocks_on() -> bool {
    FONT_BLOCKS.load(std::sync::atomic::Ordering::Relaxed)
}

fn is_block(symbol: &str) -> bool {
    matches!(symbol, "█" | "▌" | "▐" | "▀" | "▄" | "▖" | "▗" | "▘" | "▝" | "▙" | "▛" | "▜" | "▟")
}

/// Blocks as cells filled with their colour: a cell's background fills its whole height, a
/// font's block glyph may not (journal#1619). A full block always; a half block only when it stands
/// alone in its row, as a card's bar does, so a picture made of blocks, the logo, keeps its shape
/// (journal#1642).
pub fn fill_blocks(buf: &mut Buffer) {
    let width = buf.area.width as usize;
    let symbols: Vec<bool> = buf.content.iter().map(|c| is_block(c.symbol())).collect();
    for (i, cell) in buf.content.iter_mut().enumerate() {
        let x = if width == 0 { 0 } else { i % width };
        let alone = !(x > 0 && symbols[i - 1]) && !(x + 1 < width && symbols[i + 1]);
        let fill = match cell.symbol() {
            "█" => true,
            "▌" | "▐" => alone,
            _ => false,
        };
        if fill && cell.fg != Color::Reset {
            let color = cell.fg;
            cell.set_symbol(" ");
            cell.bg = color;
        }
    }
}

/// Every colour of a frame, as the terminal can show it.
pub fn fit_buffer(buf: &mut Buffer, depth: Depth) {
    if depth == Depth::TrueColor {
        return;
    }
    for cell in buf.content.iter_mut() {
        cell.fg = fit(cell.fg, depth);
        cell.bg = fit(cell.bg, depth);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| pairs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    }

    #[test]
    fn macos_terminal_gets_256_colours_and_the_rest_keep_24_bits() {
        assert_eq!(depth(env(&[("TERM_PROGRAM", "Apple_Terminal")])), Depth::Indexed256);
        assert_eq!(depth(env(&[("TERM_PROGRAM", "Apple_Terminal"), ("COLORTERM", "truecolor")])), Depth::TrueColor, "a Terminal that says it has them");
        assert_eq!(depth(env(&[("TERM_PROGRAM", "iTerm.app")])), Depth::TrueColor);
        assert_eq!(depth(env(&[])), Depth::TrueColor);
        assert_eq!(depth(env(&[("MULTITOOL_COLORS", "256"), ("COLORTERM", "truecolor")])), Depth::Indexed256, "the override wins");
        assert_eq!(depth(env(&[("MULTITOOL_COLORS", "truecolor"), ("TERM_PROGRAM", "Apple_Terminal")])), Depth::TrueColor);
    }

    #[test]
    fn each_colour_goes_to_the_nearest_of_256() {
        assert_eq!(nearest_index(0, 0, 0), 16, "black is the cube's");
        assert_eq!(nearest_index(255, 255, 255), 231);
        assert_eq!(nearest_index(255, 0, 0), 196);
        assert_eq!(nearest_index(0x80, 0x80, 0x80), 244, "a mid grey is the ramp's");
        assert_eq!(nearest_index(0xF5, 0xA6, 0x23), 214, "the brand orange");
        assert_eq!(nearest_index(0x0B, 0x0E, 0x14), 233, "the dark theme's background, a near black");
    }

    #[test]
    fn blocks_become_filled_cells_in_macos_terminal() {
        assert!(font_blocks(env(&[("TERM_PROGRAM", "Apple_Terminal")])));
        assert!(!font_blocks(env(&[("TERM_PROGRAM", "iTerm.app")])));
        // A card's bar, alone in its row, and a full block: filled.
        let mut buf = Buffer::empty(Rect::new(0, 0, 5, 1));
        for (i, s) in ["▌", " ", "█", " ", "x"].iter().enumerate() {
            buf.content[i].set_symbol(s);
            buf.content[i].fg = Color::Indexed(214);
        }
        fill_blocks(&mut buf);
        for i in [0, 2] {
            assert_eq!((buf.content[i].symbol(), buf.content[i].bg), (" ", Color::Indexed(214)));
        }
        assert_eq!(buf.content[4].symbol(), "x", "text stays");
        // journal#1642: a half block beside other blocks is part of a picture, and keeps its shape.
        let mut logo = Buffer::empty(Rect::new(0, 0, 3, 1));
        for (i, s) in ["▐", "█", "▌"].iter().enumerate() {
            logo.content[i].set_symbol(s);
            logo.content[i].fg = Color::Indexed(214);
        }
        fill_blocks(&mut logo);
        assert_eq!((logo.content[0].symbol(), logo.content[2].symbol()), ("▐", "▌"));
    }

    #[test]
    fn a_frame_keeps_no_24_bit_colour() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf.content[0].fg = Color::Rgb(0xF5, 0xA6, 0x23);
        buf.content[1].bg = Color::Rgb(10, 10, 10);
        buf.content[2].fg = Color::Red;
        let mut kept = buf.clone();
        fit_buffer(&mut kept, Depth::TrueColor);
        assert_eq!(kept, buf, "24 bits stay as they are");
        fit_buffer(&mut buf, Depth::Indexed256);
        assert!(buf.content.iter().all(|c| !matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..))));
        assert_eq!(buf.content[0].fg, Color::Indexed(214));
        assert_eq!(buf.content[2].fg, Color::Red, "a named colour stays");
    }
}
