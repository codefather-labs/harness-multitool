//! Conditions (IF-15): any logic over accumulated signals, always a bool. A project's
//! `.harness/conditions.toml` names them; a true one wakes the agent with one message per batch
//! (INV-14). Other kinds of conditions implement the same trait.

use std::path::Path;
use std::time::Duration;

use harness_api::{Callback, Condition};
use serde::Deserialize;
use serde_json::Value;

/// One `[[condition]]` of the project's file.
#[derive(Debug, Clone, Deserialize)]
pub struct ConditionConfig {
    pub name: String,
    /// The callback type it listens to.
    pub on: String,
    /// How many signals make it true.
    #[serde(default = "one")]
    pub at_least: usize,
    /// Only signals this recent count; all of them when absent.
    pub within_s: Option<u64>,
    /// After a message, no other for this long; the signals still go to the log.
    #[serde(default = "default_cooldown")]
    pub cooldown_s: u64,
    /// Only signals whose field passes this count.
    pub field: Option<String>,
    pub op: Option<String>,
    pub value: Option<Value>,
    /// The response-policy files this condition links (CON-26).
    #[serde(default)]
    pub policies: Vec<String>,
}

fn one() -> usize {
    1
}

fn default_cooldown() -> u64 {
    60
}

#[derive(Debug, Default, Deserialize)]
struct ConditionsFile {
    #[serde(default)]
    condition: Vec<ConditionConfig>,
}

/// The shipped kind: enough signals, of those that pass the optional field rule.
pub struct CountCondition(pub ConditionConfig);

fn passes(signal: &Value, field: &str, op: &str, want: &Value) -> bool {
    let got = &signal[field];
    match op {
        "eq" => got == want,
        "ne" => got != want,
        "gt" | "lt" | "ge" | "le" => match (got.as_f64(), want.as_f64()) {
            (Some(a), Some(b)) => match op {
                "gt" => a > b,
                "lt" => a < b,
                "ge" => a >= b,
                _ => a <= b,
            },
            _ => false,
        },
        _ => false,
    }
}

impl Condition for CountCondition {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn check(&self, signals: &[Callback]) -> bool {
        let counted = signals.iter().filter(|s| match (&self.0.field, &self.0.value) {
            (Some(field), Some(want)) => passes(&s.fields, field, self.0.op.as_deref().unwrap_or("eq"), want),
            _ => true,
        });
        counted.count() >= self.0.at_least
    }

    fn policies(&self) -> Vec<String> {
        self.0.policies.clone()
    }
}

/// A condition with what the daemon tracks for it in one window.
pub struct Armed {
    pub on: String,
    pub within: Option<Duration>,
    pub cooldown: Duration,
    pub condition: Box<dyn Condition + Send>,
    /// Signals of the batch being gathered, with when they came.
    pub signals: Vec<(u64, Callback)>,
    /// No message before this time.
    pub quiet_until_ms: u64,
    /// Signals that came during the quiet time, for the next message to mention.
    pub held: usize,
}

/// Reads a project's conditions. No file means none. A broken file is an error to show, not a
/// reason to lose the window.
pub fn load(root: &str) -> Result<Vec<Armed>, String> {
    let path = Path::new(root).join(".harness").join("conditions.toml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("reading {}: {e}", path.display())),
    };
    let file: ConditionsFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(file
        .condition
        .into_iter()
        .map(|c| Armed {
            on: c.on.clone(),
            within: c.within_s.map(Duration::from_secs),
            cooldown: Duration::from_secs(c.cooldown_s),
            condition: Box::new(CountCondition(c)),
            signals: Vec::new(),
            quiet_until_ms: 0,
            held: 0,
        })
        .collect())
}

impl Armed {
    /// Adds a signal. Returns the batch's message if the condition just fired.
    pub fn signal(&mut self, now_ms: u64, callback: Callback, log_path: &Path) -> Option<(String, String)> {
        if now_ms < self.quiet_until_ms {
            self.held += 1;
            return None;
        }
        self.signals.push((now_ms, callback));
        if let Some(within) = self.within {
            let oldest = now_ms.saturating_sub(within.as_millis() as u64);
            self.signals.retain(|(at, _)| *at >= oldest);
        }
        let batch: Vec<Callback> = self.signals.iter().map(|(_, c)| c.clone()).collect();
        if !self.condition.check(&batch) {
            return None;
        }
        let count = batch.len();
        let held = std::mem::take(&mut self.held);
        let latest = batch.last().map(|c| c.fields.to_string()).unwrap_or_default();
        self.signals.clear();
        self.quiet_until_ms = now_ms + self.cooldown.as_millis() as u64;
        let policies = self.condition.policies();
        let policy_line = if policies.is_empty() { "No response policy is linked to it.".to_string() } else { format!("Response policies: {}.", policies.join(", ")) };
        let earlier = if held > 0 { format!(" {held} more came during the quiet time after the last message.") } else { String::new() };
        let text = format!(
            "Condition \"{}\" is true: {count} signal(s) of type {} in this batch.{earlier} This is one message, not a stream: read the trigger log {} for the nearby triggers of this and similar conditions, there may be many. {policy_line} Latest signal: {latest}",
            self.condition.name(),
            self.on,
            log_path.display(),
        );
        Some((self.condition.name().to_string(), text))
    }
}
