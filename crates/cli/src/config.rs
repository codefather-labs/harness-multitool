//! The user's configuration, by profile (CON-120, ADR 0026): `~/.harness/profiles/<name>/profile.toml`
//! holds a profile's settings, `~/.harness/shared.toml` the cross-profile ones, Telegram and voice
//! (CON-124). This process runs on one profile, chosen once at its start with [`select`]; every
//! function here reads and writes that one. `default` always exists. The configuration of before
//! profiles, `<config dir>/harness-multitool/config.toml`, is copied into `default` once and left
//! where it is, for older builds still running (journal#754).

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
    /// The input device for the page's test, none for the system's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_name: Option<String>,
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

/// What a profile's file holds: every setting but the cross-profile ones.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    terminal: Terminal,
    #[serde(default)]
    session: harness_session::Settings,
}

/// What `shared.toml` holds: the same in every profile (CON-124).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SharedFile {
    #[serde(default)]
    voice: Voice,
    #[serde(default)]
    telegram: Telegram,
}

pub const DEFAULT_PROFILE: &str = "default";
const MAX_NAME: usize = 32;

static CURRENT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Where every profile and the shared settings live: `~/.harness`, or `MULTITOOL_HOME` when it is
/// set, which tests use so they never touch the user's own (journal#751).
pub fn root() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os("MULTITOOL_HOME").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(dirs::home_dir().ok_or_else(|| anyhow!("this OS reports no home directory"))?.join(".harness"))
}

/// The configuration file of before profiles, read only to move it into `default`.
fn old_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("harness-multitool").join("config.toml"))
}

/// A profile's name, if it may be one: lowercase letters, digits and `-`, at most 32 (CON-120).
pub fn check_name(name: &str) -> Result<()> {
    let fits = !name.is_empty() && name.len() <= MAX_NAME && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if fits { Ok(()) } else { Err(anyhow!("a profile's name is lowercase letters, digits and -, at most {MAX_NAME}: {name:?} is not")) }
}

/// The profile this process runs on.
pub fn current() -> &'static str {
    CURRENT.get().map_or(DEFAULT_PROFILE, String::as_str)
}

/// Runs this process on `name`, once, at its start. An unknown name is refused with the names
/// there are (CON-121).
pub fn select(name: &str) -> Result<()> {
    let root = root()?;
    ensure(&root, old_path().as_deref())?;
    check_name(name)?;
    let names = list_in(&root)?;
    if !names.iter().any(|n| n == name) {
        return Err(anyhow!("no profile {name:?}; the profiles are: {}", names.join(", ")));
    }
    CURRENT.set(name.to_string()).map_err(|_| anyhow!("the profile is chosen once, at the start"))
}

/// The profiles there are, `default` first, then by name.
#[allow(dead_code)] // The Profiles tab calls it (journal#749).
pub fn list() -> Result<Vec<String>> {
    let root = root()?;
    ensure(&root, old_path().as_deref())?;
    list_in(&root)
}

fn list_in(root: &std::path::Path) -> Result<Vec<String>> {
    let dir = root.join("profiles");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("profile.toml").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| check_name(n).is_ok())
        .collect();
    names.sort_by_key(|n| (n != DEFAULT_PROFILE, n.clone()));
    Ok(names)
}

/// Where this process's profile keeps its ChatGPT credential (CON-122). `default`'s stays where it
/// was before profiles: moving or copying it would leave an older build, or a second copy of a
/// rotating refresh token, behind (journal#754).
pub fn chatgpt_dir() -> Result<PathBuf> {
    if current() == DEFAULT_PROFILE {
        return Ok(data_dir()?.join("auth").join("chatgpt"));
    }
    Ok(profile_dir(current())?.join("chatgpt"))
}

/// The Claude Code configuration directory of this process's profile: none for `default`, which
/// runs on the user's own Claude Code login (CON-120).
pub fn claude_dir() -> Result<Option<PathBuf>> {
    Ok((current() != DEFAULT_PROFILE).then(|| profile_dir(current())).transpose()?.map(|d| d.join("claude")))
}

/// What this window's sessions must see of its profile: the folders of its sign-ins, by the
/// variables the delegate and the auth plugin read (CON-122). Paths only, never a credential.
pub fn sign_in_environment() -> Result<Vec<(&'static str, PathBuf)>> {
    let mut vars = vec![(harness_session::CHATGPT_DIR_VAR, chatgpt_dir()?)];
    if let Some(dir) = claude_dir()? {
        private_dir(&dir)?;
        vars.push(("CLAUDE_CONFIG_DIR", dir));
    }
    Ok(vars)
}

/// A profile's folder.
pub fn profile_dir(name: &str) -> Result<PathBuf> {
    check_name(name)?;
    Ok(root()?.join("profiles").join(name))
}

/// Makes a new profile, entirely clean (CON-125): the built-in defaults, no key, no sign-in, and an
/// empty Claude Code directory of its own.
#[allow(dead_code)] // The Profiles tab calls it (journal#749).
pub fn create(name: &str) -> Result<PathBuf> {
    let root = root()?;
    ensure(&root, old_path().as_deref())?;
    create_in(&root, name)
}

fn create_in(root: &std::path::Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let dir = root.join("profiles").join(name);
    if dir.exists() {
        return Err(anyhow!("a profile {name:?} is there already"));
    }
    private_dir(&dir)?;
    private_dir(&dir.join("claude"))?;
    write_private(&dir.join("profile.toml"), &toml::to_string_pretty(&ProfileFile::default())?)?;
    Ok(dir)
}

/// The first start with profiles: `default` and `shared.toml` from the configuration of before, if
/// there was one. That file is left as it is: an older build still running reads it, and this one
/// never does again, since `default` exists (journal#754). Later starts change nothing.
fn ensure(root: &std::path::Path, old: Option<&std::path::Path>) -> Result<()> {
    let default = root.join("profiles").join(DEFAULT_PROFILE);
    if default.join("profile.toml").is_file() {
        return Ok(());
    }
    let before = match old.map(|p| (p, std::fs::read_to_string(p))) {
        Some((p, Ok(text))) => Some((p, parse(p, &text)?)),
        Some((p, Err(e))) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).with_context(|| format!("reading {}", p.display())),
        _ => None,
    };
    let c = before.as_ref().map(|(_, c)| c.clone()).unwrap_or_default();
    private_dir(root)?;
    private_dir(&root.join("profiles"))?;
    private_dir(&default)?;
    if !root.join("shared.toml").is_file() {
        write_private(&root.join("shared.toml"), &toml::to_string_pretty(&SharedFile { voice: c.voice.clone(), telegram: c.telegram.clone() })?)?;
    }
    write_private(&default.join("profile.toml"), &toml::to_string_pretty(&ProfileFile { terminal: c.terminal, session: c.session })?)?;
    Ok(())
}

fn private_dir(dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).with_context(|| format!("securing {}", dir.display()))?;
    }
    Ok(())
}

/// Written whole, owner-only: these files can hold keys and a bot token.
fn write_private(p: &std::path::Path, text: &str) -> Result<()> {
    let temp = p.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600)).with_context(|| format!("securing {}", temp.display()))?;
    }
    std::fs::rename(&temp, p).with_context(|| format!("writing {}", p.display()))
}

/// This process's profile's file.
pub fn path() -> Result<PathBuf> {
    Ok(profile_dir(current())?.join("profile.toml"))
}

/// The configuration of this process's profile, with the shared settings.
pub fn load() -> Result<Config> {
    let root = root()?;
    ensure(&root, old_path().as_deref())?;
    load_in(&root, current())
}

fn load_in(root: &std::path::Path, name: &str) -> Result<Config> {
    let p = root.join("profiles").join(name).join("profile.toml");
    let profile: ProfileFile = read(&p)?;
    let shared: SharedFile = read(&root.join("shared.toml"))?;
    Ok(Config { terminal: profile.terminal, voice: shared.voice, telegram: shared.telegram, session: profile.session })
}

/// A file's settings, the defaults when it is not there.
fn read<T: Default + serde::de::DeserializeOwned>(p: &std::path::Path) -> Result<T> {
    match std::fs::read_to_string(p) {
        Ok(text) => toml::from_str(&text).map_err(|e| anyhow!("{} is not valid TOML{}", p.display(), line_of(&e, &text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e).with_context(|| format!("reading {}", p.display())),
    }
}

/// The file's text as a configuration. An error names the path and the line and nothing of the text:
/// a parse error quotes the line, and this file holds tokens (INV-6, journal#696).
fn parse(p: &std::path::Path, text: &str) -> Result<Config> {
    toml::from_str(text).map_err(|e| anyhow::anyhow!("{} is not valid TOML{}", p.display(), line_of(&e, text)))
}

/// Where a parse error is, as words to follow a sentence.
fn line_of(e: &toml::de::Error, text: &str) -> String {
    e.span().map(|s| format!(" (line {})", text[..s.start.min(text.len())].matches('\n').count() + 1)).unwrap_or_default()
}

/// The `map_max_nodes` setting, and a sentence when the file could not be read so the default is used instead
/// (journal#631). The sentence holds the path and a line number only: a parse error quotes the line, and the
/// file holds tokens (INV-6).
pub fn map_limit() -> (u64, Option<String>) {
    let default = harness_session::Settings::default().map_max_nodes;
    if let Err(e) = root().and_then(|root| ensure(&root, old_path().as_deref())) {
        return (default, Some(format!("{e}; the default map node limit {default} is used.")));
    }
    let Ok(p) = path() else { return (default, None) };
    match std::fs::read_to_string(&p) {
        Ok(text) => map_limit_from_text(&p, &text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (default, None),
        Err(_) => (default, Some(format!("{} cannot be read; the default map node limit {default} is used.", p.display()))),
    }
}

/// The `search_max_linked` setting; the default when the file cannot be read, which `map_limit` tells (CON-118).
pub fn search_page() -> u64 {
    load().map_or(harness_session::Settings::default().search_max_linked, |c| c.session.search_max_linked)
}

fn map_limit_from_text(p: &std::path::Path, text: &str) -> (u64, Option<String>) {
    match toml::from_str::<Config>(text) {
        Ok(c) => (c.session.map_max_nodes, None),
        Err(e) => {
            let default = harness_session::Settings::default().map_max_nodes;
            (default, Some(format!("{} is not valid TOML{}; the default map node limit {default} is used.", p.display(), line_of(&e, text))))
        }
    }
}

/// Where sessions keep their logs: the OS's data directory.
pub fn data_dir() -> Result<PathBuf> {
    let dir = dirs::data_dir().ok_or_else(|| anyhow!("this OS reports no data directory"))?;
    Ok(dir.join("harness-multitool"))
}

/// Saves the settings to this process's profile, and the shared ones to `shared.toml` (CON-121, CON-124).
pub fn save(c: &Config) -> Result<PathBuf> {
    let root = root()?;
    ensure(&root, old_path().as_deref())?;
    save_in(&root, current(), c)
}

fn save_in(root: &std::path::Path, name: &str, c: &Config) -> Result<PathBuf> {
    let dir = root.join("profiles").join(name);
    private_dir(&dir)?;
    let p = dir.join("profile.toml");
    write_private(&p, &toml::to_string_pretty(&ProfileFile { terminal: c.terminal.clone(), session: c.session.clone() })?)?;
    write_private(&root.join("shared.toml"), &toml::to_string_pretty(&SharedFile { voice: c.voice.clone(), telegram: c.telegram.clone() })?)?;
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

    /// journal#631, INV-6: a broken file is told by its path and line, never by the error text, which quotes the
    /// line and so a token in it; the default limit is used.
    #[test]
    fn a_broken_config_is_told_without_quoting_its_text() {
        let path = std::path::Path::new("/x/config.toml");
        let text = "[telegram]\ntoken = \"CANARY-SECRET-123\" oops\n";
        let (limit, message) = map_limit_from_text(path, text);
        let message = message.expect("a message");
        assert_eq!(limit, 200);
        assert!(!message.contains("CANARY") && !message.contains("oops"), "{message}");
        assert!(message.contains("/x/config.toml") && message.contains("line 2"), "{message}");
        assert_eq!(map_limit_from_text(path, "[session]\nmap_max_nodes = 42\n"), (42, None));
        assert_eq!(map_limit_from_text(path, ""), (200, None));
        // journal#696: so for every command that reads the file, cause chain included.
        let error = format!("{:?}", parse(path, text).expect_err("an error"));
        assert!(!error.contains("CANARY") && !error.contains("oops"), "{error}");
        assert!(error.contains("/x/config.toml") && error.contains("line 2"), "{error}");
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("multitool-profiles-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[cfg(unix)]
    fn mode(p: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// CON-120, CON-124: the configuration of before is copied into `default` once, Telegram and
    /// voice into `shared.toml`; the old file stays, and a second start changes nothing.
    #[test]
    fn the_old_config_moves_into_default_once() {
        let root = temp_root("migrate");
        let old_dir = root.join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        let old = old_dir.join("config.toml");
        std::fs::write(&old, "[terminal]\ngraphics = \"sixel\"\n[telegram]\ntoken = \"CANARY-T\"\nallowed_users = [7]\n[voice]\nbackend = \"whisper\"\n[session]\nmap_max_nodes = 42\n").unwrap();
        let home = root.join("home");
        let before = std::fs::read(&old).unwrap();
        ensure(&home, Some(&old)).unwrap();
        assert_eq!(std::fs::read(&old).unwrap(), before, "the old file stays as it was, for older builds (journal#754)");
        let c = load_in(&home, DEFAULT_PROFILE).unwrap();
        assert_eq!((c.terminal.graphics, c.session.map_max_nodes, c.telegram.allowed_users.clone(), c.voice.backend.as_deref()), (Graphics::Sixel, 42, vec![7], Some("whisper")));
        let profile = std::fs::read_to_string(home.join("profiles/default/profile.toml")).unwrap();
        assert!(!profile.contains("CANARY") && !profile.contains("telegram"), "{profile}");
        assert!(std::fs::read_to_string(home.join("shared.toml")).unwrap().contains("CANARY-T"));
        #[cfg(unix)]
        {
            assert_eq!(mode(&home.join("shared.toml")), 0o600);
            assert_eq!(mode(&home.join("profiles/default/profile.toml")), 0o600);
            assert_eq!(mode(&home.join("profiles/default")), 0o700);
        }
        // A second start: the old file is not read again, even when an older build changed it.
        std::fs::write(&old, "[session]\nmap_max_nodes = 9\n").unwrap();
        ensure(&home, Some(&old)).unwrap();
        assert_eq!(load_in(&home, DEFAULT_PROFILE).unwrap().session.map_max_nodes, 42);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CON-120: a fresh install has `default` with the built-in settings and no old file is needed.
    #[test]
    fn a_fresh_install_starts_with_default() {
        let root = temp_root("fresh");
        ensure(&root, Some(&root.join("absent.toml"))).unwrap();
        assert_eq!(list_in(&root).unwrap(), vec![DEFAULT_PROFILE.to_string()]);
        assert_eq!(load_in(&root, DEFAULT_PROFILE).unwrap().session.map_max_nodes, 200);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CON-125: a new profile is clean, has its own empty Claude Code directory, and its settings are
    /// its own; the shared ones are the same in every profile.
    #[test]
    fn a_new_profile_is_clean_and_its_settings_its_own() {
        let root = temp_root("create");
        ensure(&root, None).unwrap();
        let mut c = load_in(&root, DEFAULT_PROFILE).unwrap();
        c.session.map_max_nodes = 7;
        c.telegram.allowed_users = vec![5];
        save_in(&root, DEFAULT_PROFILE, &c).unwrap();
        let dir = create_in(&root, "work").unwrap();
        assert!(dir.join("claude").is_dir() && std::fs::read_dir(dir.join("claude")).unwrap().next().is_none());
        let work = load_in(&root, "work").unwrap();
        assert_eq!(work.session.map_max_nodes, 200, "nothing copied from default");
        assert_eq!(work.telegram.allowed_users, vec![5], "Telegram is cross-profile");
        assert_eq!(list_in(&root).unwrap(), vec!["default".to_string(), "work".to_string()]);
        assert!(create_in(&root, "work").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn profile_names_are_checked() {
        for good in ["default", "work", "a-1", &"x".repeat(32)] {
            assert!(check_name(good).is_ok(), "{good}");
        }
        for bad in ["", "Work", "a/b", "..", "a b", &"x".repeat(33), "café"] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    /// journal#696 holds for a profile's file: its error names the path and the line, no text.
    #[test]
    fn a_broken_profile_is_told_without_quoting_its_text() {
        let root = temp_root("broken");
        ensure(&root, None).unwrap();
        std::fs::write(root.join("profiles/default/profile.toml"), "[session]\nmodel = \"CANARY-K\" oops\n").unwrap();
        let error = format!("{:?}", load_in(&root, DEFAULT_PROFILE).expect_err("an error"));
        assert!(!error.contains("CANARY") && !error.contains("oops") && error.contains("line 2"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// CON-114: the node limit is saved in `[session]`, read back, and 200 when the file says nothing.
    #[test]
    fn the_map_node_limit_round_trips_and_defaults_to_200() {
        assert_eq!(toml::from_str::<Config>("").unwrap().session.map_max_nodes, 200);
        let mut c = Config::default();
        c.session.map_max_nodes = 0;
        let text = toml::to_string_pretty(&c).unwrap();
        assert!(text.contains("map_max_nodes = 0"), "{text}");
        assert_eq!(toml::from_str::<Config>(&text).unwrap().session.map_max_nodes, 0);
        assert_eq!(toml::from_str::<Config>("[session]\nmap_max_nodes = 42\n").unwrap().session.map_max_nodes, 42);
    }
}
