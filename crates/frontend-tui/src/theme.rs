//! Theme tokens: the sixteen semantic roles plus the brand colour, loaded from `themes/*.toml`.

use ratatui::style::Color;
use serde::Deserialize;

/// The themes shipped with the binary, by file name.
pub const BUILTIN: &[(&str, &str)] = &[
    ("dark", include_str!("../../../themes/dark.toml")),
    ("light", include_str!("../../../themes/light.toml")),
    ("nord", include_str!("../../../themes/nord.toml")),
    ("monokai", include_str!("../../../themes/monokai.toml")),
    ("cyan", include_str!("../../../themes/cyan.toml")),
];

#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub dark: bool,
    pub background: Color,
    pub surface: Color,
    pub surface_raised: Color,
    pub overlay_scrim: Color,
    pub border_subtle: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub diff_add_bg: Color,
    pub diff_del_bg: Color,
    pub selection: Color,
    pub brand: Color,
}

#[derive(Deserialize)]
struct File {
    name: String,
    kind: String,
    background: String,
    surface: String,
    surface_raised: String,
    overlay_scrim: String,
    border_subtle: String,
    text: String,
    text_muted: String,
    accent: String,
    accent_text: String,
    success: String,
    warning: String,
    error: String,
    info: String,
    diff_add_bg: String,
    diff_del_bg: String,
    selection: String,
    brand: String,
}

#[derive(Debug)]
pub struct ThemeError(pub String);

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ThemeError {}

fn hex(field: &str, value: &str) -> Result<Color, ThemeError> {
    let v = value.strip_prefix('#').unwrap_or(value);
    if v.len() != 6 {
        return Err(ThemeError(format!("{field}: expected #RRGGBB, got {value:?}")));
    }
    let n = u32::from_str_radix(v, 16)
        .map_err(|_| ThemeError(format!("{field}: not hex: {value:?}")))?;
    Ok(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

impl Theme {
    pub fn parse(id: &str, toml_text: &str) -> Result<Theme, ThemeError> {
        let f: File = toml::from_str(toml_text).map_err(|e| ThemeError(format!("{id}: {e}")))?;
        Ok(Theme {
            id: id.to_string(),
            name: f.name,
            dark: f.kind == "dark",
            background: hex("background", &f.background)?,
            surface: hex("surface", &f.surface)?,
            surface_raised: hex("surface_raised", &f.surface_raised)?,
            overlay_scrim: hex("overlay_scrim", &f.overlay_scrim)?,
            border_subtle: hex("border_subtle", &f.border_subtle)?,
            text: hex("text", &f.text)?,
            text_muted: hex("text_muted", &f.text_muted)?,
            accent: hex("accent", &f.accent)?,
            accent_text: hex("accent_text", &f.accent_text)?,
            success: hex("success", &f.success)?,
            warning: hex("warning", &f.warning)?,
            error: hex("error", &f.error)?,
            info: hex("info", &f.info)?,
            diff_add_bg: hex("diff_add_bg", &f.diff_add_bg)?,
            diff_del_bg: hex("diff_del_bg", &f.diff_del_bg)?,
            selection: hex("selection", &f.selection)?,
            brand: hex("brand", &f.brand)?,
        })
    }

    /// A built-in theme by id (`dark`, `light`, `nord`, `monokai`, `cyan`).
    pub fn builtin(id: &str) -> Result<Theme, ThemeError> {
        let (_, text) = BUILTIN
            .iter()
            .find(|(n, _)| *n == id)
            .ok_or_else(|| ThemeError(format!("unknown theme {id:?}")))?;
        Theme::parse(id, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_theme_parses() {
        for (id, _) in BUILTIN {
            let t = Theme::builtin(id).unwrap();
            assert_eq!(t.brand, Color::Rgb(0xF5, 0xA6, 0x23), "{id}: brand is the same in every theme");
        }
    }

    #[test]
    fn dark_background_is_the_token_value() {
        assert_eq!(Theme::builtin("dark").unwrap().background, Color::Rgb(0x0B, 0x0E, 0x14));
    }
}
