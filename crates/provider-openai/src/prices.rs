//! OpenAI's API prices, per 1M tokens: (input, output). Standard tier, short-context rate, from
//! developers.openai.com/api/docs/pricing, read 2026-11-01. The API does not list prices, so a
//! model missing here shows no price rather than a guessed one.

const TABLE: [(&str, f64, f64); 32] = [
    ("gpt-6-astra", 10.0, 50.0),
    ("gpt-6.1-sol", 2.0, 10.0),
    ("gpt-6.1-terra", 1.0, 5.0),
    ("gpt-6-luna", 0.10, 0.50),
    ("gpt-6-sol", 2.0, 10.0),
    ("gpt-5.6-sol", 4.0, 20.0),
    ("gpt-5.6-terra", 2.0, 12.0),
    ("gpt-5.6-luna", 0.20, 1.20),
    ("gpt-5.5", 5.0, 30.0),
    ("gpt-5.5-pro", 30.0, 180.0),
    ("gpt-5.4", 2.50, 15.0),
    ("gpt-5.4-pro", 30.0, 180.0),
    ("gpt-5.4-mini", 0.75, 4.50),
    ("gpt-5.4-nano", 0.20, 1.25),
    ("gpt-5.3-codex", 1.75, 14.0),
    ("gpt-5.2", 1.75, 14.0),
    ("gpt-5.2-pro", 21.0, 168.0),
    ("gpt-5.1", 1.25, 10.0),
    ("gpt-5", 1.25, 10.0),
    ("gpt-5-mini", 0.25, 2.0),
    ("gpt-5-nano", 0.05, 0.40),
    ("gpt-5-pro", 15.0, 120.0),
    ("gpt-4.1", 2.0, 8.0),
    ("gpt-4.1-mini", 0.40, 1.60),
    ("gpt-4.1-nano", 0.10, 0.40),
    ("gpt-4o", 2.50, 10.0),
    ("gpt-4o-mini", 0.15, 0.60),
    ("o1", 15.0, 60.0),
    ("o3", 2.0, 8.0),
    ("o3-pro", 20.0, 80.0),
    ("o3-mini", 1.10, 4.40),
    ("o4-mini", 1.10, 4.40),
];

pub fn per_million(model: &str) -> Option<(f64, f64)> {
    TABLE.iter().find(|(id, ..)| *id == model).map(|(_, input, output)| (*input, *output))
}
