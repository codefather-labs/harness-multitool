//! Design-debug tool: draws an `AppState` through the real `frontend_tui::draw` into an off-screen
//! ratatui buffer and rasterises that buffer to an image, cell by cell. No terminal is involved.
//! The live TUI never depends on this crate.

use std::path::Path;

use frontend_tui::images::{self, Placement};
use frontend_tui::{AppState, Theme};
use fontdue::{Font, FontSettings};
use image::{Rgb, RgbImage, imageops};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// Fonts are compiled in, so a render is the same on Linux, macOS and Windows (licences next to
/// them in `fonts/`). `Raster::with_fonts` takes other files instead.
const MONO: &[u8] = include_bytes!("../../frontend-tui/fonts/DejaVuSansMono.ttf");
const MONO_BOLD: &[u8] = include_bytes!("../../frontend-tui/fonts/DejaVuSansMono-Bold.ttf");
/// Tried in order for characters the monospace font lacks, as a terminal falls back.
const FALLBACK: &[&[u8]] = &[include_bytes!("../../frontend-tui/fonts/DejaVuSans.ttf"), include_bytes!("../../frontend-tui/fonts/NotoSansSymbols2-Regular.ttf")];

fn parse(name: &str, bytes: Vec<u8>) -> Result<Font, Error> {
    Font::from_bytes(bytes, FontSettings::default()).map_err(|e| Error(format!("{name}: {e}")))
}

/// Cell geometry and fonts used for rasterising.
pub struct Raster {
    regular: Font,
    bold: Font,
    /// Fonts tried in order for characters the monospace font lacks, as a terminal would.
    fallback: Vec<Font>,
    pub cell_w: u32,
    pub cell_h: u32,
    pub px: f32,
}

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn load(path: &Path) -> Result<Font, Error> {
    let bytes = std::fs::read(path).map_err(|e| Error(format!("{}: {e}", path.display())))?;
    parse(&path.display().to_string(), bytes)
}

impl Raster {
    /// The built-in fonts. `px` is the font size; the cell width is the advance of `M` at that
    /// size, rounded.
    pub fn new(px: f32, cell_h: u32) -> Result<Raster, Error> {
        let regular = parse("DejaVuSansMono", MONO.to_vec())?;
        let bold = parse("DejaVuSansMono-Bold", MONO_BOLD.to_vec())?;
        Self::build(regular, bold, px, cell_h)
    }

    /// Other font files for the regular and bold faces; the built-in fallbacks stay.
    pub fn with_fonts(regular: &Path, bold: &Path, px: f32, cell_h: u32) -> Result<Raster, Error> {
        Self::build(load(regular)?, load(bold)?, px, cell_h)
    }

    fn build(regular: Font, bold: Font, px: f32, cell_h: u32) -> Result<Raster, Error> {
        let cell_w = regular.metrics('M', px).advance_width.round() as u32;
        let fallback = FALLBACK.iter().enumerate().map(|(i, b)| parse(&format!("fallback {i}"), b.to_vec())).collect::<Result<_, _>>()?;
        Ok(Raster { regular, bold, fallback, cell_w, cell_h, px })
    }

    /// Draws the state at `cols x rows` cells and returns the picture.
    /// As a terminal with graphics shows it (pictures pasted over their cells).
    pub fn render(&self, state: &AppState, theme: &Theme, cols: u16, rows: u16) -> RgbImage {
        self.render_with(state, theme, cols, rows, true)
    }

    /// `graphics: false` shows the text fallbacks, as a terminal without graphics does.
    pub fn render_with(&self, state: &AppState, theme: &Theme, cols: u16, rows: u16, graphics: bool) -> RgbImage {
        let mut term = Terminal::new(TestBackend::new(cols, rows)).expect("test backend");
        let mut placements = Vec::new();
        let state = AppState { pictures: graphics, ..state.clone() };
        term.draw(|f| placements = frontend_tui::draw(f, &state, theme).placements).expect("draw");
        let buf = term.backend().buffer();
        let mut img = self.rasterise(buf, theme);
        if graphics {
            for p in &placements {
                self.paste(&mut img, buf, theme, p);
            }
        }
        img
    }

    /// A picture over its cells: the cells' background first (the text fallback is not shown,
    /// as a graphics-capable terminal would not show it), then the rasterised graphic.
    fn paste(&self, img: &mut RgbImage, buf: &Buffer, theme: &Theme, p: &Placement) {
        let (cw, ch) = (self.cell_w, self.cell_h);
        let (x0, y0) = (p.rect.x as u32 * cw, p.rect.y as u32 * ch);
        let (w, h) = (p.rect.width as u32 * cw, p.rect.height as u32 * ch);
        let bg = rgb(buf[(p.rect.x, p.rect.y)].bg, theme.background);
        let pic = images::rasterize(&p.graphic, theme, w, h);
        for (x, y, px) in pic.enumerate_pixels() {
            let (ix, iy) = (x0 + x, y0 + y);
            if ix < img.width() && iy < img.height() {
                img.put_pixel(ix, iy, blend(bg, Rgb([px[0], px[1], px[2]]), px[3]));
            }
        }
    }

    pub fn rasterise(&self, buf: &Buffer, theme: &Theme) -> RgbImage {
        let (cw, ch) = (self.cell_w, self.cell_h);
        let area = buf.area;
        let mut img = RgbImage::from_pixel(area.width as u32 * cw, area.height as u32 * ch, rgb(theme.background, theme.background));
        let ascent = self.regular.horizontal_line_metrics(self.px).map(|m| m.ascent).unwrap_or(self.px * 0.8);
        let baseline = ((ch as f32 - self.px * 1.17) / 2.0 + ascent).round();

        for y in 0..area.height {
            for x in 0..area.width {
                let cell = &buf[(x, y)];
                let (mut fg, mut bg) = (rgb(cell.fg, theme.text), rgb(cell.bg, theme.background));
                if cell.modifier.contains(Modifier::REVERSED) {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let (ox, oy) = (x as u32 * cw, y as u32 * ch);
                for py in oy..oy + ch {
                    for px in ox..ox + cw {
                        img.put_pixel(px, py, bg);
                    }
                }
                // Underline: two pixels just under the baseline, as terminals draw it.
                if cell.modifier.contains(Modifier::UNDERLINED) {
                    let uy = oy + (baseline as u32 + 2).min(ch - 2);
                    for py in uy..uy + 2 {
                        for px in ox..ox + cw {
                            img.put_pixel(px, py, fg);
                        }
                    }
                }
                let sym = cell.symbol();
                let Some(c) = sym.chars().next() else { continue };
                if c == ' ' {
                    continue;
                }
                if shapes::draw(&mut img, c, ox, oy, cw, ch, fg) {
                    continue;
                }
                let mut font = if cell.modifier.contains(Modifier::BOLD) { &self.bold } else { &self.regular };
                if font.lookup_glyph_index(c) == 0 {
                    if let Some(f) = self.fallback.iter().find(|f| f.lookup_glyph_index(c) != 0) {
                        font = f;
                    }
                }
                let (m, bitmap) = font.rasterize(c, self.px);
                let gx = ox as i32 + m.xmin;
                let gy = oy as i32 + baseline as i32 - m.height as i32 - m.ymin;
                for (i, a) in bitmap.iter().enumerate() {
                    if *a == 0 {
                        continue;
                    }
                    let (px, py) = (gx + (i % m.width) as i32, gy + (i / m.width) as i32);
                    if px < 0 || py < 0 || px as u32 >= img.width() || py as u32 >= img.height() {
                        continue;
                    }
                    let under = *img.get_pixel(px as u32, py as u32);
                    img.put_pixel(px as u32, py as u32, blend(under, fg, *a));
                }
            }
        }
        img
    }
}

/// `left | right`, the right one scaled to the left one's height, with a gap.
pub fn side_by_side(left: &RgbImage, right: &RgbImage) -> RgbImage {
    let h = left.height();
    let w = (right.width() as f32 * h as f32 / right.height() as f32).round() as u32;
    let right = imageops::resize(right, w, h, imageops::FilterType::Triangle);
    let gap = 16;
    let mut out = RgbImage::from_pixel(left.width() + gap + w, h, Rgb([255, 0, 255]));
    imageops::replace(&mut out, left, 0, 0);
    imageops::replace(&mut out, &right, (left.width() + gap) as i64, 0);
    out
}

fn rgb(c: Color, default: Color) -> Rgb<u8> {
    let c = if c == Color::Reset { default } else { c };
    match c {
        Color::Rgb(r, g, b) => Rgb([r, g, b]),
        // Screens only use theme colours (Rgb). Anything else is a bug worth seeing.
        _ => Rgb([255, 0, 255]),
    }
}

fn blend(under: Rgb<u8>, over: Rgb<u8>, a: u8) -> Rgb<u8> {
    let a = a as u32;
    let mix = |u: u8, o: u8| ((u as u32 * (255 - a) + o as u32 * a) / 255) as u8;
    Rgb([mix(under[0], over[0]), mix(under[1], over[1]), mix(under[2], over[2])])
}

/// Box-drawing and block characters drawn as geometry, so they join across cells the way a
/// terminal draws them, whatever the font.
mod shapes {
    use image::{Rgb, RgbImage};

    fn rect(img: &mut RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, c: Rgb<u8>) {
        for y in y0..y1.min(img.height()) {
            for x in x0..x1.min(img.width()) {
                img.put_pixel(x, y, c);
            }
        }
    }

    /// Quarter-circle arc for rounded corners. `(cx, cy)` is the arc centre, `r` its radius;
    /// `qx`/`qy` choose the quadrant (+1 right/down, -1 left/up).
    fn arc(img: &mut RgbImage, cx: f32, cy: f32, r: f32, qx: f32, qy: f32, w: f32, c: Rgb<u8>) {
        let steps = (r * 4.0) as i32 + 8;
        for i in 0..=steps {
            let a = std::f32::consts::FRAC_PI_2 * i as f32 / steps as f32;
            let (x, y) = (cx + qx * r * a.cos(), cy + qy * r * a.sin());
            let half = w / 2.0;
            rect(img, (x - half).max(0.0) as u32, (y - half).max(0.0) as u32, (x + half).ceil() as u32, (y + half).ceil() as u32, c);
        }
    }

    pub fn draw(img: &mut RgbImage, ch: char, ox: u32, oy: u32, cw: u32, chh: u32, c: Rgb<u8>) -> bool {
        let (mx, my) = (ox + cw / 2, oy + chh / 2);
        let thin = 2u32;
        let thick = 4u32;
        let (x1, y1) = (ox + cw, oy + chh);
        match ch {
            '─' => rect(img, ox, my - thin / 2, x1, my - thin / 2 + thin, c),
            '━' => rect(img, ox, my - thick / 2, x1, my - thick / 2 + thick, c),
            '│' => rect(img, mx - thin / 2, oy, mx - thin / 2 + thin, y1, c),
            '┃' => rect(img, mx - thick / 2, oy, mx - thick / 2 + thick, y1, c),
            '█' => rect(img, ox, oy, x1, y1, c),
            '▌' => rect(img, ox, oy, ox + cw / 2, y1, c),
            '▐' => rect(img, ox + cw / 2, oy, x1, y1, c),
            '▀' => rect(img, ox, oy, x1, oy + chh / 2, c),
            '▄' => rect(img, ox, oy + chh / 2, x1, y1, c),
            '▏' => rect(img, ox, oy, ox + cw / 8 + 1, y1, c),
            '▕' => rect(img, x1 - cw / 8 - 1, oy, x1, y1, c),
            '▁' => rect(img, ox, y1 - chh / 8, x1, y1, c),
            '▔' => rect(img, ox, oy, x1, oy + chh / 8, c),
            '▗' => rect(img, ox + cw / 2, oy + chh / 2, x1, y1, c),
            '▖' => rect(img, ox, oy + chh / 2, ox + cw / 2, y1, c),
            '▝' => rect(img, ox + cw / 2, oy, x1, oy + chh / 2, c),
            '▘' => rect(img, ox, oy, ox + cw / 2, oy + chh / 2, c),
            '╭' | '╮' | '╰' | '╯' => {
                let r = (cw as f32 / 2.0).min(chh as f32 / 2.0);
                let (qx, qy, hx0, hx1, vy0, vy1) = match ch {
                    '╭' => (-1.0, -1.0, mx as f32 + r, x1 as f32, my as f32 + r, y1 as f32),
                    '╮' => (1.0, -1.0, ox as f32, mx as f32 - r, my as f32 + r, y1 as f32),
                    '╰' => (-1.0, 1.0, mx as f32 + r, x1 as f32, oy as f32, my as f32 - r),
                    _ => (1.0, 1.0, ox as f32, mx as f32 - r, oy as f32, my as f32 - r),
                };
                let cx = mx as f32 - qx * r;
                let cy = my as f32 - qy * r;
                arc(img, cx, cy, r, qx, qy, thin as f32, c);
                rect(img, hx0 as u32, my - thin / 2, hx1 as u32, my - thin / 2 + thin, c);
                rect(img, mx - thin / 2, vy0 as u32, mx - thin / 2 + thin, vy1 as u32, c);
            }
            // Sextants (U+1FB00..U+1FB3B): 2 x 3 blocks; the codes skip empty, ▌, ▐ and full.
            '\u{1FB00}'..='\u{1FB3B}' => {
                let i = ch as u32 - 0x1FB00 + 1;
                let mask = if i < 21 { i } else if i < 41 { i + 1 } else { i + 2 };
                let xs = [ox, ox + cw / 2, x1];
                let ys = [oy, oy + chh / 3, oy + chh * 2 / 3, y1];
                for bit in 0..6 {
                    if mask & (1 << bit) != 0 {
                        let (cx, cy) = (bit % 2, bit / 2);
                        rect(img, xs[cx], ys[cy], xs[cx + 1], ys[cy + 1], c);
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
