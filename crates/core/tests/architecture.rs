//! INV-1: the core and the interfaces touch no platform directly. INV-7: plugin crates depend
//! only on interface crates. `cargo check --target wasm32-unknown-unknown` is the other half.

use std::fs;
use std::path::Path;

const FORBIDDEN: &[&str] = &["std::fs", "std::process", "std::net", "std::thread", "std::env", "tokio", "SystemTime", "Instant::now"];

fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn core_and_api_touch_no_platform() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut files = Vec::new();
    sources(&crates.join("core/src"), &mut files);
    sources(&crates.join("api/src"), &mut files);
    assert!(files.len() >= 5, "found the sources");
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        for bad in FORBIDDEN {
            let hit = text.lines().find(|l| !l.trim_start().starts_with("//") && l.contains(bad));
            assert!(hit.is_none(), "{} uses {bad}: {hit:?}", file.display());
        }
    }
}

#[test]
fn doubles_depend_only_on_the_interfaces() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let manifest = fs::read_to_string(crates.join("doubles/Cargo.toml")).unwrap();
    let local: Vec<_> = manifest.lines().filter(|l| l.contains("path = ")).collect();
    assert_eq!(local.len(), 1, "{local:?}");
    assert!(local[0].starts_with("harness-api"));
}

/// INV-7: plugin crates depend only on interface crates, never on each other.
#[test]
fn plugins_depend_only_on_the_interfaces() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for name in [
        "platform-native",
        "auth-apikey",
        "provider-anthropic",
        "provider-openai",
        "tools-builtin",
        "policy-default",
        "context-default",
        "session-files",
        "verifier-protocol",
        "router-default",
        "hooks-shell",
    ] {
        let manifest = fs::read_to_string(crates.join(name).join("Cargo.toml")).unwrap();
        let local: Vec<_> = manifest.lines().filter(|l| l.contains("path = ")).collect();
        assert_eq!(local.len(), 1, "{name}: {local:?}");
        assert!(local[0].starts_with("harness-api"), "{name}: {}", local[0]);
    }
}

/// CON-40: the repository does not name the project it grew out of.
#[test]
fn the_sources_do_not_name_their_origin() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let origin = ["claude", "base"].concat();
    let mut files = Vec::new();
    sources(crates, &mut files);
    assert!(files.len() > 20);
    for file in files.iter().filter(|f| !f.components().any(|c| c.as_os_str() == "target")) {
        let text = fs::read_to_string(file).unwrap();
        assert!(!text.to_lowercase().contains(&origin), "{} names it", file.display());
    }
}
