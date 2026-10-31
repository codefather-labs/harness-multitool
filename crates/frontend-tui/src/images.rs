//! Graphics drawn as pictures rather than cells. `view::draw` reserves their cells (with a text
//! fallback in them) and returns a `Placement` for each; the frontend shows the picture there
//! through the terminal's graphics protocol, the snapshot tool pastes it into the PNG. Both get
//! the pixels from `rasterize`, from the same SVG source (journal: vector rule).

use image::RgbaImage;
use ratatui::layout::Rect;
use ratatui::style::Color;
use resvg::{tiny_skia, usvg};

use crate::theme::Theme;

const MARK_SVG: &str = include_str!("../../../docs/design/logo/multitool-mark.svg");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Graphic {
    /// The multitool mark in the header.
    Logo,
}

/// Where a graphic goes, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub rect: Rect,
    pub graphic: Graphic,
}

fn hex(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        _ => "#FF00FF".into(),
    }
}

/// The SVG source of a graphic, coloured for the theme. The header mark is one colour, the brand
/// orange (as in the mockups); its rivets are cut out in the theme's background.
pub fn svg(g: Graphic, t: &Theme) -> String {
    match g {
        Graphic::Logo => MARK_SVG.replace("BACKGROUND", &hex(t.background)).replace("#F5A623", &hex(t.brand)),
    }
}

/// Renders the graphic to exactly `width` x `height` pixels, fitted and centred, transparent
/// around it.
pub fn rasterize(g: Graphic, t: &Theme, width: u32, height: u32) -> RgbaImage {
    let tree = usvg::Tree::from_str(&svg(g, t), &usvg::Options::default()).expect("embedded SVG parses");
    let size = tree.size();
    let scale = (width as f32 / size.width()).min(height as f32 / size.height());
    let dx = (width as f32 - size.width() * scale) / 2.0;
    let dy = (height as f32 - size.height() * scale) / 2.0;
    let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1)).expect("non-zero pixmap");
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    // tiny-skia stores premultiplied RGBA; un-premultiply for `image`.
    let mut out = RgbaImage::new(pixmap.width(), pixmap.height());
    for (px, c) in out.pixels_mut().zip(pixmap.pixels()) {
        let c = c.demultiply();
        *px = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_rasterizes_with_the_brand_handle() {
        let t = Theme::builtin("dark").unwrap();
        let img = rasterize(Graphic::Logo, &t, 320, 88);
        let brand = img.pixels().filter(|p| p[3] == 255 && p[0] == 0xF5 && p[1] == 0xA6 && p[2] == 0x23).count();
        assert!(brand > 500, "brand-orange handle pixels: {brand}");
    }
}
