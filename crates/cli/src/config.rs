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

/// Speech to text: what the settings' page chose. What is downloaded is in the data directory.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Voice {
    /// `whisper` or `parakeet`, none for off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Voice {
    /// The chosen model, or the engine's first when none of its own is named.
    pub fn chosen(&self) -> Option<harness_speech::Model> {
        let engine = harness_speech::Engine::from_id(self.backend.as_deref()?)?;
        let named = self.model.as_deref().and_then(harness_speech::model).filter(|m| m.engine == engine);
        named.or_else(|| harness_speech::MODELS.into_iter().find(|m| m.engine == engine))
    }
}

/// The Telegram channel: off with no token. The token is the harness's own bot's, better kept in
/// MULTITOOL_TELEGRAM_TOKEN than here. `allowed_users` is who may reach an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Telegram {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub allowed_users: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub terminal: Terminal,
    #[serde(default)]
    pub voice: Voice,
    #[serde(default)]
    pub telegram: Telegram,
    /// The model and plugins a session runs on: provider, model, budgets, keys (never required
    /// here: keys normally come from the environment).
    #[serde(default)]
    pub session: harness_session::Settings,
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

/// Where sessions keep their logs: the OS's data directory.
pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir().ok_or_else(|| anyhow!("this OS reports no data directory"))?;
    Ok(dir.join("harness-multitool"))
}

pub fn save(c: &Config) -> Result<PathBuf> {
    let p = path()?;
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&p, toml::to_string_pretty(c)?).with_context(|| format!("writing {}", p.display()))?;
    // It can hold a bot token.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).with_context(|| format!("securing {}", p.display()))?;
    }
    Ok(p)
}

/// What turns a voice note into words, if one is chosen and downloaded.
fn transcriber() -> Option<harness_daemon::telegram::Transcriber> {
    let chosen = load().map(|c| c.voice).unwrap_or_default().chosen()?;
    let data = data_dir().ok()?;
    harness_speech::ready(&data, chosen).then(|| -> harness_daemon::telegram::Transcriber { std::sync::Arc::new(move |note: &[u8]| harness_speech::transcribe(&data, chosen, note)) })
}

/// The Telegram channel's settings, if it has a token: the environment wins over the file.
pub fn telegram() -> Option<harness_daemon::telegram::Config> {
    let file = load().map(|c| c.telegram).unwrap_or_default();
    let token = std::env::var("MULTITOOL_TELEGRAM_TOKEN").ok().filter(|t| !t.is_empty()).or(file.token)?;
    Some(harness_daemon::telegram::Config {
        base_url: file.base_url.unwrap_or_else(|| "https://api.telegram.org".to_string()),
        token: harness_api::Secret::new(token),
        allowed_users: file.allowed_users,
        chat: file.chat,
        poll_timeout_s: 25,
        transcriber: transcriber(),
    })
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
        let c = Config { terminal: Terminal { graphics: Graphics::Sixel }, ..Default::default() };
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("graphics = \"sixel\""), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap().terminal.graphics, Graphics::Sixel);
    }
}
