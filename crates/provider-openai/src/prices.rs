//! OpenAI's API prices, per 1M tokens: (input, cached input, output). Standard tier, short-context rate, from
//! developers.openai.com/api/docs/pricing, read 2026-11-01. The API does not list prices, so a
//! model missing here shows no price rather than a guessed one.

const TABLE: [(&str, f64, f64, f64); 32] = [
    ("gpt-6-astra", 10.0, 1.0, 50.0),
    ("gpt-6.1-sol", 2.0, 0.1, 10.0),
    ("gpt-6.1-terra", 1.0, 0.05, 5.0),
    ("gpt-6-luna", 0.10, 0.01, 0.50),
    ("gpt-6-sol", 2.0, 0.2, 10.0),
    ("gpt-5.6-sol", 4.0, 0.4, 20.0),
    ("gpt-5.6-terra", 2.0, 0.2, 12.0),
    ("gpt-5.6-luna", 0.20, 0.02, 1.20),
    ("gpt-5.5", 5.0, 0.5, 30.0),
    ("gpt-5.5-pro", 30.0, 30.0, 180.0),
    ("gpt-5.4", 2.50, 0.25, 15.0),
    ("gpt-5.4-pro", 30.0, 30.0, 180.0),
    ("gpt-5.4-mini", 0.75, 0.075, 4.50),
    ("gpt-5.4-nano", 0.20, 0.02, 1.25),
    ("gpt-5.3-codex", 1.75, 0.175, 14.0),
    ("gpt-5.2", 1.75, 0.175, 14.0),
    ("gpt-5.2-pro", 21.0, 21.0, 168.0),
    ("gpt-5.1", 1.25, 0.125, 10.0),
    ("gpt-5", 1.25, 0.125, 10.0),
    ("gpt-5-mini", 0.25, 0.025, 2.0),
    ("gpt-5-nano", 0.05, 0.005, 0.40),
    ("gpt-5-pro", 15.0, 15.0, 120.0),
    ("gpt-4.1", 2.0, 0.5, 8.0),
    ("gpt-4.1-mini", 0.40, 0.1, 1.60),
    ("gpt-4.1-nano", 0.10, 0.025, 0.40),
    ("gpt-4o", 2.50, 1.25, 10.0),
    ("gpt-4o-mini", 0.15, 0.075, 0.60),
    ("o1", 15.0, 7.5, 60.0),
    ("o3", 2.0, 0.5, 8.0),
    ("o3-pro", 20.0, 20.0, 80.0),
    ("o3-mini", 1.10, 0.55, 4.40),
    ("o4-mini", 1.10, 0.275, 4.40),
];

pub fn per_million(model: &str) -> Option<(f64, f64)> {
    TABLE.iter().find(|(id, ..)| *id == model).map(|(_, input, _, output)| (*input, *output))
}

/// What a turn costs on the API, in micro-dollars; `None` for a model not in the table.
pub fn cost_micro_usd(model: &str, usage: &harness_api::Usage) -> Option<u64> {
    let (_, input, cached, output) = TABLE.iter().find(|(id, ..)| *id == model)?;
    Some((usage.input as f64 * input + usage.cache_read as f64 * cached + usage.output as f64 * output).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_costs_what_the_table_says_and_an_unknown_model_nothing_known() {
        let usage = harness_api::Usage { input: 1_000_000, cache_read: 1_000_000, output: 100_000, cache_write: 0 };
        assert_eq!(cost_micro_usd("gpt-6.1-sol", &usage), Some(3_100_000), "$2 + $0.10 + $1");
        assert_eq!(cost_micro_usd("no-such-model", &usage), None);
        assert_eq!(per_million("gpt-6-luna"), Some((0.10, 0.50)));
    }
}
