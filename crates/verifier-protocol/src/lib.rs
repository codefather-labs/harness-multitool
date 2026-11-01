//! Runs a task's protocol (IF-10): each step is a command that must exit 0. The output is the
//! evidence. No processes, or no steps, means no proof: it fails (INV-4).

use std::rc::Rc;

use harness_api::platform::Process;
use harness_api::*;

pub struct ProtocolRunner {
    process: Option<Rc<dyn Process>>,
    root: String,
}

impl ProtocolRunner {
    pub fn new(process: Option<Rc<dyn Process>>, root: &str) -> Self {
        ProtocolRunner { process, root: root.to_string() }
    }
}

fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

impl Verifier for ProtocolRunner {
    fn verify<'a>(&'a self, protocol: &'a Protocol, _state: &'a SessionState) -> BoxFuture<'a, PluginResult<Verification>> {
        Box::pin(async move {
            if protocol.steps.is_empty() {
                return Ok(Verification { pass: false, evidence: "the protocol has no steps, so nothing was proven".into() });
            }
            let Some(process) = &self.process else {
                return Ok(Verification { pass: false, evidence: "this platform has no processes to run the protocol".into() });
            };
            let mut evidence = Vec::new();
            for step in &protocol.steps {
                let out = process.run(step, Some(&self.root)).await?;
                let pass = out.code == Some(0);
                evidence.push(format!("$ {step}\nexit {}\n{}{}", out.code.map_or("none".to_string(), |c| c.to_string()), tail(&out.stdout), tail(&out.stderr)));
                if !pass {
                    return Ok(Verification { pass: false, evidence: evidence.join("\n") });
                }
            }
            Ok(Verification { pass: true, evidence: evidence.join("\n") })
        })
    }
}
