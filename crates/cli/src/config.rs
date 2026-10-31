//! The user's configuration: `<config dir>/harness-multitool/config.toml`, where the config dir
//! is the OS's own (dirs::config_dir: ~/.config on Linux, ~/Library/Application Support on
//! macOS, %APPDATA% on Windows). Written by `multitool setup`; every field has a default.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Graphics {
    /// Ask the terminal when its environment suggests graphics (the default).
    #[default]
    Auto,
    Sixel,
    Kitty,
    Iterm2,
    /// Never draw pictures; text fallbacks only.
    Off,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Terminal {
    #[serde(default)]
    pub graphics: Graphics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub terminal: Terminal,
}

pub fn path() -> Result<PathBuf> {
    let dir = dirs::config_dir().ok_or_else(|| anyhow!("this OS reports no config directory"))?;
    Ok(dir.join("harness-multitool").join("config.toml"))
}

/// The configuration, or the defaults when there is no file yet.
pub fn load() -> Result<Config> {
    let p = path()?;
    match std::fs::read_to_string(&p) {
        Ok(text) => toml::from_str(&text).with_context(|| format!("reading {}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

pub fn save(c: &Config) -> Result<PathBuf> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&p, toml::to_string_pretty(c)?).with_context(|| format!("writing {}", p.display()))?;
    Ok(p)
}

/// The graphics mode in effect: MULTITOOL_GRAPHICS (sixel|kitty|iterm2|auto|off, also 1=auto
/// and 0=off) overrides the config file.
pub fn graphics() -> Graphics {
    let from_env = match std::env::var("MULTITOOL_GRAPHICS").unwrap_or_default().as_str() {
        "sixel" => Some(Graphics::Sixel),
        "kitty" => Some(Graphics::Kitty),
        "iterm2" => Some(Graphics::Iterm2),
        "auto" | "1" => Some(Graphics::Auto),
        "off" | "0" => Some(Graphics::Off),
        _ => None,
    };
    from_env.unwrap_or_else(|| load().map(|c| c.terminal.graphics).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_and_defaults_to_auto() {
        assert_eq!(toml::from_str::<Config>("").unwrap().terminal.graphics, Graphics::Auto);
        let c = Config { terminal: Terminal { graphics: Graphics::Sixel } };
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("graphics = \"sixel\""), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap().terminal.graphics, Graphics::Sixel);
    }
}
