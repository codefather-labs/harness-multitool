//! One model for everything, budgets in turns and cost (IF-3).

use harness_api::{Route, Router, Spent};
use serde_json::Value;

pub struct OneModel {
    route: Route,
    max_turns: Option<u32>,
    max_cost_micro_usd: Option<u64>,
}

impl OneModel {
    /// Settings: `provider`, `model`, `max_tokens`, `max_turns`, `max_cost_usd`.
    pub fn new(settings: &Value) -> Result<Self, String> {
        let text = |k: &str| settings[k].as_str().map(str::to_string).ok_or_else(|| format!("the router needs {k} in the configuration"));
        Ok(OneModel {
            route: Route { provider: text("provider")?, model: text("model")?, max_tokens: settings["max_tokens"].as_u64().unwrap_or(64_000) as u32 },
            max_turns: settings["max_turns"].as_u64().map(|n| n as u32),
            max_cost_micro_usd: settings["max_cost_usd"].as_f64().map(|d| (d * 1e6) as u64),
        })
    }
}

impl Router for OneModel {
    fn route(&self, spent: &Spent) -> Result<Route, String> {
        if let Some(max) = self.max_turns.filter(|max| spent.turns >= *max) {
            return Err(format!("budget exhausted: {max} turns"));
        }
        if let Some(max) = self.max_cost_micro_usd.filter(|max| spent.cost_micro_usd >= *max) {
            return Err(format!("budget exhausted: ${:.2}", max as f64 / 1e6));
        }
        Ok(self.route.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harness_api::Usage;
    use serde_json::json;

    #[test]
    fn budgets_stop_it_and_missing_settings_are_named() {
        let r = OneModel::new(&json!({"provider":"p","model":"m","max_turns":2,"max_cost_usd":1.0})).unwrap();
        let spent = |turns, cost_micro_usd| Spent { turns, usage: Usage::default(), cost_micro_usd, elapsed_ms: 0 };
        assert!(r.route(&spent(1, 0)).is_ok());
        assert!(r.route(&spent(2, 0)).unwrap_err().contains("2 turns"));
        assert!(r.route(&spent(1, 1_000_000)).unwrap_err().contains("$1.00"));
        assert!(OneModel::new(&json!({})).err().unwrap().contains("provider"));
    }
}
