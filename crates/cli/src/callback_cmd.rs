//! `multitool callback`: a project's custom callbacks (CON-183, journal#1373). A handler is a Rust
//! program on the harness-callback crate; `new` makes one from the template, `build` builds it.
//! The crate goes with this binary and is written to `<data dir>/sdk/harness-callback`, so a
//! handler is always built on the contract of the multitool that runs it.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

const SDK_LIB: &str = include_str!("../../callback-sdk/src/lib.rs");
const TEMPLATE_CARGO: &str = include_str!("../../callback-sdk/template/Cargo.toml");
const TEMPLATE_MAIN: &str = include_str!("../../callback-sdk/template/main.rs");
const TEMPLATE_CONFIG: &str = include_str!("../../callback-sdk/template/config.toml");

fn sdk_manifest() -> String {
    format!(
        "[package]\nname = \"harness-callback\"\nversion = \"{}\"\nedition = \"2024\"\ndescription = \"The contract of a multitool custom callback handler (CON-183).\"\n\n[dependencies]\nserde_json = \"1\"\nureq = {{ version = \"3\", features = [\"json\"] }}\n\n[workspace]\n",
        env!("CARGO_PKG_VERSION")
    )
}

/// Writes the crate where handlers find it, when it is missing or another build's.
pub fn ensure_sdk(data: &Path) -> Result<PathBuf> {
    let dir = data.join("sdk").join("harness-callback");
    std::fs::create_dir_all(dir.join("src")).with_context(|| format!("making {}", dir.display()))?;
    for (path, text) in [(dir.join("Cargo.toml"), sdk_manifest()), (dir.join("src/lib.rs"), SDK_LIB.to_string())] {
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(dir)
}

/// A name a folder, a Cargo package and a systemd unit all take.
fn check_name(name: &str) -> Result<()> {
    let ok = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') && name.starts_with(|c: char| c.is_ascii_lowercase());
    if !ok {
        bail!("a callback's name is lowercase letters, digits, - and _, starting with a letter: {name:?}");
    }
    Ok(())
}

fn folder(root: &Path, name: &str) -> PathBuf {
    root.join(".harness").join("callbacks").join(name)
}

fn fill(template: &str, name: &str, sdk: &Path) -> String {
    template.replace("{{name}}", name).replace("{{sdk}}", &toml_text(&sdk.to_string_lossy()))
}

/// Text as it goes inside a TOML basic string: a Windows path's backslashes would be escapes there
/// and break the Cargo.toml (journal#1584).
fn toml_text(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Makes `.harness/callbacks/<name>/` from the template; never over an existing folder.
pub fn new(root: &Path, data: &Path, name: &str, types: &[String]) -> Result<PathBuf> {
    check_name(name)?;
    let dir = folder(root, name);
    if dir.exists() {
        bail!("{} exists already", dir.display());
    }
    let sdk = ensure_sdk(data)?;
    std::fs::create_dir_all(dir.join("src"))?;
    let mut config = fill(TEMPLATE_CONFIG, name, &sdk);
    if !types.is_empty() {
        let list = types.iter().map(|t| format!("{t:?}")).collect::<Vec<_>>().join(", ");
        config = config.replace(&format!("types = [\"{name}\"]"), &format!("types = [{list}]"));
    }
    std::fs::write(dir.join("Cargo.toml"), fill(TEMPLATE_CARGO, name, &sdk))?;
    std::fs::write(dir.join("src/main.rs"), fill(TEMPLATE_MAIN, name, &sdk))?;
    std::fs::write(dir.join("config.toml"), config)?;
    std::fs::write(dir.join(".gitignore"), "/target\n")?;
    Ok(dir)
}

/// `cargo build --release` in the callback's folder, its output to this terminal.
pub fn build(root: &Path, data: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let dir = folder(root, name);
    if !dir.join("Cargo.toml").is_file() {
        bail!("{} has no Cargo.toml: make it with multitool callback new {name}", dir.display());
    }
    ensure_sdk(data)?;
    let status = std::process::Command::new("cargo").args(["build", "--release"]).current_dir(&dir).status().context("running cargo: is the Rust toolchain installed?")?;
    if !status.success() {
        bail!("cargo build failed in {}", dir.display());
    }
    Ok(dir.join("target").join("release").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// journal#1373: the scaffold makes a project on the crate as this binary carries it.
    #[test]
    fn a_new_callback_is_a_rust_project_on_the_sdk() {
        let (root, data) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        assert!(new(root.path(), data.path(), "Bad Name", &[]).is_err());
        let dir = new(root.path(), data.path(), "deploy-done", &["deploy_done".into(), "deploy_failed".into()]).unwrap();
        let cargo = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        let sdk = data.path().join("sdk").join("harness-callback");
        assert!(cargo.parse::<toml::Table>().is_ok(), "the Cargo.toml is TOML on every OS: {cargo}");
        assert!(cargo.contains("name = \"deploy-done\"") && cargo.contains(&format!("path = \"{}\"", toml_text(&sdk.to_string_lossy()))), "{cargo}");
        let config = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(config.contains("types = [\"deploy_done\", \"deploy_failed\"]") && config.contains("handler = \"target/release/deploy-done\""), "{config}");
        assert!(std::fs::read_to_string(dir.join("src/main.rs")).unwrap().contains("harness_callback::run"));
        assert_eq!(std::fs::read_to_string(sdk.join("src/lib.rs")).unwrap(), SDK_LIB);
        assert!(new(root.path(), data.path(), "deploy-done", &[]).is_err(), "never over an existing folder");
        let found = harness_daemon::project_callbacks::found(&root.path().to_string_lossy());
        assert_eq!(found[0].types, ["deploy_done", "deploy_failed"]);
        assert!(found[0].problem.as_deref().unwrap().contains("not an executable"), "unbuilt yet: {:?}", found[0].problem);
    }
}
