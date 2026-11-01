//! The default policy (IF-5): reads inside the project run; writes, commands and network ask;
//! an "always" answer holds for the same tool and capability within the session.

use std::cell::RefCell;
use std::collections::HashSet;

use harness_api::{Answer, Capability, Decision, Policy, ToolCall};

pub struct DefaultPolicy {
    root: String,
    remembered: RefCell<HashSet<(String, Capability)>>,
}

impl DefaultPolicy {
    pub fn new(root: &str) -> Self {
        DefaultPolicy { root: root.trim_end_matches('/').to_string(), remembered: RefCell::default() }
    }

    fn inside_project(&self, path: &str) -> bool {
        let root = normalize(&self.root);
        // A relative path is relative to the project; `..` still counts.
        let joined;
        let path = if path.starts_with('/') || path.get(1..3) == Some(":\\") {
            path
        } else {
            joined = format!("{}/{path}", self.root);
            &joined
        };
        let path = normalize(path);
        path.len() >= root.len() && path[..root.len()] == root[..]
    }
}

/// A path's parts with `.` and `..` resolved, without touching the file system.
fn normalize(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts
}

impl Policy for DefaultPolicy {
    fn decide(&self, call: &ToolCall, capabilities: &[Capability]) -> Decision {
        let remembered = self.remembered.borrow();
        let settled = |c: &Capability| {
            remembered.contains(&(call.name.clone(), c.clone()))
                || match c {
                    Capability::ReadPath(p) => self.inside_project(p),
                    Capability::Memory => true,
                    _ => false,
                }
        };
        if capabilities.iter().all(settled) { Decision::Allow } else { Decision::Ask }
    }

    fn remember(&self, call: &ToolCall, capabilities: &[Capability], answer: Answer) {
        if answer == Answer::AlwaysAllow {
            let mut remembered = self.remembered.borrow_mut();
            for c in capabilities {
                remembered.insert((call.name.clone(), c.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str) -> ToolCall {
        ToolCall { id: "c".into(), name: name.into(), input: json!({}) }
    }

    #[test]
    fn reads_inside_run_and_everything_else_asks() {
        let p = DefaultPolicy::new("/work/app");
        let d = |c: Capability| p.decide(&call("t"), &[c]);
        assert_eq!(d(Capability::ReadPath("/work/app/src/a.rs".into())), Decision::Allow);
        assert_eq!(d(Capability::ReadPath("src/a.rs".into())), Decision::Allow);
        assert_eq!(d(Capability::ReadPath(".".into())), Decision::Allow);
        assert_eq!(d(Capability::ReadPath("../other/a.rs".into())), Decision::Ask);
        assert_eq!(d(Capability::ReadPath("/work/app/../secret".into())), Decision::Ask, "dot-dot does not escape");
        assert_eq!(d(Capability::ReadPath("/work/application/x".into())), Decision::Ask, "a prefix is not a parent");
        assert_eq!(d(Capability::WritePath("/work/app/a.rs".into())), Decision::Ask);
        assert_eq!(d(Capability::Command("ls".into())), Decision::Ask);
        assert_eq!(d(Capability::Host("example.com".into())), Decision::Ask);
    }

    #[test]
    fn always_allow_holds_for_that_tool_and_capability_only() {
        let p = DefaultPolicy::new("/work");
        let c = [Capability::Command("cargo test".into())];
        p.remember(&call("shell"), &c, Answer::AllowOnce);
        assert_eq!(p.decide(&call("shell"), &c), Decision::Ask, "once is once");
        p.remember(&call("shell"), &c, Answer::AlwaysAllow);
        assert_eq!(p.decide(&call("shell"), &c), Decision::Allow);
        assert_eq!(p.decide(&call("shell"), &[Capability::Command("rm -rf /".into())]), Decision::Ask);
        assert_eq!(p.decide(&call("other"), &c), Decision::Ask);
    }
}
