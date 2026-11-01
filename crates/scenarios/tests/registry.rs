//! IF-0.3 and INV-2.

use harness_core::{Registry, RegistryError, Selection};
use harness_doubles::platform;
use std::rc::Rc;

#[test]
fn an_unknown_name_is_an_error_naming_what_is_known() {
    let mut registry = Registry::new();
    registry.auth("api-key", |_| Ok(Rc::new(harness_doubles::FixedAuth(harness_api::Secret::new("x")))));
    let selection = Selection { auth: "subscription".into(), ..Default::default() };
    let err = registry.resolve(&selection, platform()).err().expect("unknown name");
    assert_eq!(err, RegistryError::Unknown { interface: "Auth", name: "subscription".into(), known: vec!["api-key".into()] });
    assert!(err.to_string().contains("known: api-key"));
}

/// INV-2: the loop runs from a selection of doubles; this crate links no default plugin.
#[test]
fn the_loop_runs_on_doubles_alone() {
    let manifest = include_str!("../Cargo.toml");
    for line in manifest.lines().filter(|l| l.starts_with("harness-")) {
        let name = line.split_whitespace().next().unwrap();
        assert!(["harness-api", "harness-core", "harness-doubles"].contains(&name), "{name}");
    }
    let t = harness_scenarios::run(harness_scenarios::setup("plain").unwrap());
    assert_eq!(t.state.turns, 1);
}
