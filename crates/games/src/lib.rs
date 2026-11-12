//! Games between agents (ADR 0029, journal#952). The rules live here, pure: no platform, no clock,
//! randomness from a seed, so a game is the same from its seed and its actions. The daemon holds
//! the tables and tells each agent what concerns it; agents act through tools.

pub mod holdem;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A player: an agent, by its nick.
pub type PlayerId = String;

/// Something that happened at a table, for one player (`to`) or for everyone (`to` None).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub to: Option<PlayerId>,
    /// What kind: started, cards, your_turn, acted, board, hand_ended, game_over.
    pub kind: String,
    /// In words, for the agent's callback.
    pub text: String,
    /// The same, for a program: the dashboard.
    #[serde(default)]
    pub data: Value,
}

/// A game, as the daemon runs it.
pub trait Game {
    fn name(&self) -> &str;
    /// Who has to act now, if anyone.
    fn to_act(&self) -> Option<PlayerId>;
    /// A player's action, as the game's tool gives it. Refused, saying why, when it is not legal.
    fn act(&mut self, player: &str, action: &Value) -> Result<Vec<Event>, String>;
    /// The player to act did not, in time: what the rules do for them.
    fn timeout(&mut self) -> Vec<Event>;
    /// What one player may see (their cards, never another's); None for a spectator.
    fn view(&self, player: Option<&str>) -> Value;
    fn over(&self) -> bool;
}

/// A tiny seeded generator (xorshift64*): the same seed, the same deck.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn draw(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.draw() % (i as u64 + 1)) as usize;
            items.swap(i, j);
        }
    }
}
