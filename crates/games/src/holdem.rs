//! No-limit Texas hold'em (ADR 0029): blinds and the button, four betting rounds, side pots, the
//! showdown with the best five of seven, split pots. Play money only.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{Event, Game, PlayerId, Rng};

/// A card: rank * 4 + suit, rank 0 a deuce up to 12 an ace.
pub type Card = u8;

const RANKS: &[u8; 13] = b"23456789TJQKA";
const SUITS: [&str; 4] = ["♠", "♥", "♦", "♣"];

pub fn card_name(card: Card) -> String {
    format!("{}{}", RANKS[(card / 4) as usize] as char, SUITS[(card % 4) as usize])
}

fn cards_name(cards: &[Card]) -> String {
    if cards.is_empty() { "none".to_string() } else { cards.iter().map(|c| card_name(*c)).collect::<Vec<_>>().join(" ") }
}

/// The rules of a table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub stack: u64,
    pub small_blind: u64,
    pub big_blind: u64,
    /// The game ends after this many hands at most.
    pub max_hands: u32,
}

impl Default for Rules {
    fn default() -> Self {
        Rules { stack: 1000, small_blind: 10, big_blind: 20, max_hands: 50 }
    }
}

/// How good a hand is: its category (8 straight flush down to 0 high card), then its ranks in the
/// order they are compared. Bigger is better.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HandRank(pub u8, pub Vec<u8>);

impl HandRank {
    pub fn name(&self) -> &'static str {
        ["high card", "a pair", "two pair", "three of a kind", "a straight", "a flush", "a full house", "four of a kind", "a straight flush"][self.0 as usize]
    }
}

/// The rank of exactly five cards.
pub fn rank5(cards: &[Card; 5]) -> HandRank {
    let mut ranks: Vec<u8> = cards.iter().map(|c| c / 4).collect();
    ranks.sort_unstable_by(|a, b| b.cmp(a));
    let flush = cards.iter().all(|c| c % 4 == cards[0] % 4);
    let distinct = {
        let mut d = ranks.clone();
        d.dedup();
        d
    };
    let straight_high = if distinct.len() == 5 && ranks[0] - ranks[4] == 4 {
        Some(ranks[0])
    } else if distinct == [12, 3, 2, 1, 0] {
        Some(3) // the wheel: five high
    } else {
        None
    };
    // Groups by count, then by rank.
    let mut groups: Vec<(u8, u8)> = Vec::new();
    for r in &ranks {
        match groups.iter_mut().find(|(rank, _)| rank == r) {
            Some(g) => g.1 += 1,
            None => groups.push((*r, 1)),
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.cmp(&a.0)));
    let by_groups: Vec<u8> = groups.iter().map(|(r, _)| *r).collect();
    match (straight_high, flush, groups[0].1, groups.get(1).map_or(0, |g| g.1)) {
        (Some(high), true, _, _) => HandRank(8, vec![high]),
        (_, _, 4, _) => HandRank(7, by_groups),
        (_, _, 3, 2) => HandRank(6, by_groups),
        (_, true, _, _) => HandRank(5, ranks),
        (Some(high), false, _, _) => HandRank(4, vec![high]),
        (_, _, 3, _) => HandRank(3, by_groups),
        (_, _, 2, 2) => HandRank(2, by_groups),
        (_, _, 2, _) => HandRank(1, by_groups),
        _ => HandRank(0, ranks),
    }
}

/// The best five of up to seven cards.
pub fn best(cards: &[Card]) -> HandRank {
    let n = cards.len();
    let mut top: Option<HandRank> = None;
    for a in 0..n {
        for b in a + 1..n {
            for c in b + 1..n {
                for d in c + 1..n {
                    for e in d + 1..n {
                        let rank = rank5(&[cards[a], cards[b], cards[c], cards[d], cards[e]]);
                        if top.as_ref().is_none_or(|t| rank > *t) {
                            top = Some(rank);
                        }
                    }
                }
            }
        }
    }
    top.unwrap_or(HandRank(0, Vec::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Street {
    Preflop,
    Flop,
    Turn,
    River,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Seat {
    id: PlayerId,
    stack: u64,
    /// Put in this betting round.
    round: u64,
    /// Put in this hand, for the pots.
    hand: u64,
    hole: Vec<Card>,
    folded: bool,
    all_in: bool,
    /// Acted since the bet was last raised.
    acted: bool,
    /// Out of chips: out of the game.
    out: bool,
}

impl Seat {
    fn live(&self) -> bool {
        !self.out && !self.folded
    }
    fn can_act(&self) -> bool {
        self.live() && !self.all_in
    }
}

/// A pot: its amount, and the seats that may win it.
pub type Pot = (u64, Vec<usize>);

/// A table of hold'em.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holdem {
    rules: Rules,
    seats: Vec<Seat>,
    seed: u64,
    hand_no: u32,
    button: usize,
    deck: Vec<Card>,
    /// Cards fixed for a test, dealt before the shuffled deck.
    #[serde(default)]
    stacked: Vec<Card>,
    board: Vec<Card>,
    street: Street,
    to_act: Option<usize>,
    current_bet: u64,
    min_raise: u64,
    over: bool,
    /// The small and big blinds' seats of this hand, set at its deal (CON-150).
    #[serde(default)]
    sb: usize,
    #[serde(default)]
    bb: usize,
}

impl Holdem {
    /// A table for these players; the first hand is dealt at once.
    /// The hand being played, from 1; 0 before the first is dealt.
    pub fn hand_no(&self) -> u32 {
        self.hand_no
    }

    pub fn new(players: &[PlayerId], seed: u64, rules: Rules) -> (Holdem, Vec<Event>) {
        Self::with_deck(players, seed, rules, Vec::new())
    }

    /// The same, dealing `stacked` first, in order: each player's two cards seat by seat from the
    /// one after the button, then the flop, the turn and the river. For tests.
    pub fn with_deck(players: &[PlayerId], seed: u64, rules: Rules, stacked: Vec<Card>) -> (Holdem, Vec<Event>) {
        let seats = players.iter().map(|id| Seat { id: id.clone(), stack: rules.stack, round: 0, hand: 0, hole: Vec::new(), folded: false, all_in: false, acted: false, out: false }).collect();
        let mut table = Holdem { rules, seats, seed, hand_no: 0, button: 0, deck: Vec::new(), stacked, board: Vec::new(), street: Street::Preflop, to_act: None, current_bet: 0, min_raise: 0, over: false, sb: 0, bb: 0 };
        let mut events = vec![Event { to: None, kind: "started".into(), text: format!("A table of Texas hold'em with {} players, stacks of {}, blinds {}/{}.", players.len(), table.rules.stack, table.rules.small_blind, table.rules.big_blind), data: json!({"players": players, "rules": table.rules}) }];
        events.extend(table.start_hand());
        (table, events)
    }

    fn active(&self) -> Vec<usize> {
        (0..self.seats.len()).filter(|i| !self.seats[*i].out).collect()
    }

    /// The next seat after `from` that is still in the game, or that can act when `acting`.
    fn next(&self, from: usize, acting: bool) -> Option<usize> {
        let n = self.seats.len();
        (1..=n).map(|k| (from + k) % n).find(|i| if acting { self.seats[*i].can_act() } else { !self.seats[*i].out })
    }

    fn draw(&mut self) -> Card {
        if !self.stacked.is_empty() {
            let card = self.stacked.remove(0);
            self.deck.retain(|c| *c != card);
            return card;
        }
        self.deck.pop().expect("a deck of 52 never runs out in one hand")
    }

    fn pay(&mut self, seat: usize, amount: u64) -> u64 {
        let s = &mut self.seats[seat];
        let paid = amount.min(s.stack);
        s.stack -= paid;
        s.round += paid;
        s.hand += paid;
        if s.stack == 0 {
            s.all_in = true;
        }
        paid
    }

    fn pot(&self) -> u64 {
        self.seats.iter().map(|s| s.hand).sum()
    }

    fn start_hand(&mut self) -> Vec<Event> {
        self.hand_no += 1;
        let mut deck: Vec<Card> = (0..52).collect();
        Rng::new(self.seed ^ u64::from(self.hand_no).wrapping_mul(0x1000_0001)).shuffle(&mut deck);
        self.deck = deck;
        for c in self.stacked.clone() {
            self.deck.retain(|d| *d != c);
        }
        self.board.clear();
        self.street = Street::Preflop;
        for s in &mut self.seats {
            s.round = 0;
            s.hand = 0;
            s.hole.clear();
            s.folded = false;
            s.all_in = false;
            s.acted = false;
        }
        let active = self.active();
        let heads_up = active.len() == 2;
        let sb = if heads_up { self.button } else { self.next(self.button, false).expect("two players at least") };
        let bb = self.next(sb, false).expect("two players at least");
        (self.sb, self.bb) = (sb, bb);
        let mut events = Vec::new();
        // Two cards each, from the seat after the button.
        let order: Vec<usize> = {
            let mut o = Vec::new();
            let mut at = self.button;
            for _ in 0..active.len() {
                at = self.next(at, false).expect("players");
                o.push(at);
            }
            o
        };
        for &seat in order.iter().chain(order.iter()) {
            let card = self.draw();
            self.seats[seat].hole.push(card);
        }
        let (small, big) = (self.rules.small_blind, self.rules.big_blind);
        self.pay(sb, small);
        self.pay(bb, big);
        self.current_bet = big;
        self.min_raise = big;
        events.push(Event { to: None, kind: "hand".into(), text: format!("Hand {}: the button is {}; {} posts {small}, {} posts {big}.", self.hand_no, self.seats[self.button].id, self.seats[sb].id, self.seats[bb].id), data: json!({"hand": self.hand_no, "button": self.seats[self.button].id}) });
        for &seat in &order {
            events.push(Event { to: Some(self.seats[seat].id.clone()), kind: "cards".into(), text: format!("Hand {}: your cards are {}.", self.hand_no, cards_name(&self.seats[seat].hole)), data: json!({"cards": self.seats[seat].hole}) });
        }
        // Preflop, the first to act is after the big blind (the small blind heads-up).
        self.to_act = if heads_up { Some(sb).filter(|s| self.seats[*s].can_act()).or_else(|| self.next(sb, true)) } else { self.next(bb, true) };
        events.extend(self.settle());
        events
    }

    fn to_call(&self, seat: usize) -> u64 {
        self.current_bet.saturating_sub(self.seats[seat].round)
    }

    fn legal(&self, seat: usize) -> Vec<&'static str> {
        let s = &self.seats[seat];
        let mut out = vec!["fold"];
        if self.to_call(seat) == 0 {
            out.push("check");
            if s.stack > 0 {
                out.push("bet");
            }
        } else {
            out.push("call");
            if s.stack > self.to_call(seat) {
                out.push("raise");
            }
        }
        if s.stack > 0 {
            out.push("all_in");
        }
        out
    }

    fn your_turn(&self) -> Option<Event> {
        let seat = self.to_act?;
        let s = &self.seats[seat];
        let to_call = self.to_call(seat);
        let min_to = self.current_bet + self.min_raise;
        Some(Event {
            to: Some(s.id.clone()),
            kind: "your_turn".into(),
            text: format!(
                "Your turn, hand {} ({}): you hold {}, the board is {}, the pot is {}, {} to call; your stack is {}. You may {}{}. Act with game_act.",
                self.hand_no,
                serde_json::to_value(self.street).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
                cards_name(&s.hole),
                cards_name(&self.board),
                self.pot(),
                to_call,
                s.stack,
                self.legal(seat).join(", "),
                if self.legal(seat).iter().any(|a| *a == "bet" || *a == "raise") { format!(" (a bet or raise to at least {min_to})") } else { String::new() }
            ),
            data: self.view(Some(&s.id)),
        })
    }

    /// After an action: the hand ends, the round ends, or the next player is asked.
    fn settle(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let live: Vec<usize> = (0..self.seats.len()).filter(|i| self.seats[*i].live()).collect();
        if live.len() == 1 {
            let winner = live[0];
            let pot = self.pot();
            self.seats[winner].stack += pot;
            events.push(Event { to: None, kind: "hand_ended".into(), text: format!("Hand {}: {} wins {pot}; everyone else folded.", self.hand_no, self.seats[winner].id), data: json!({"hand": self.hand_no, "winners": [{"player": self.seats[winner].id, "won": pot}]}) });
            events.extend(self.end_hand());
            return events;
        }
        let acting: Vec<usize> = live.iter().copied().filter(|i| self.seats[*i].can_act()).collect();
        let round_done = acting.iter().all(|i| self.seats[*i].acted && self.seats[*i].round == self.current_bet) && !(acting.len() == 1 && self.seats[acting[0]].round < self.current_bet);
        if !round_done {
            // The player to act stays if set and still able; else the next who can.
            if self.to_act.is_none_or(|s| !self.seats[s].can_act() || (self.seats[s].acted && self.seats[s].round == self.current_bet)) {
                self.to_act = self.next(self.to_act.unwrap_or(self.button), true);
            }
            events.extend(self.your_turn());
            return events;
        }
        // The round is over: the next street, or the showdown.
        for s in &mut self.seats {
            s.round = 0;
            s.acted = false;
        }
        self.current_bet = 0;
        self.min_raise = self.rules.big_blind;
        let next = match self.street {
            Street::Preflop => Some((Street::Flop, 3)),
            Street::Flop => Some((Street::Turn, 1)),
            Street::Turn => Some((Street::River, 1)),
            Street::River => None,
        };
        let Some((street, count)) = next else {
            events.extend(self.showdown());
            return events;
        };
        self.street = street;
        for _ in 0..count {
            let card = self.draw();
            self.board.push(card);
        }
        events.push(Event { to: None, kind: "board".into(), text: format!("Hand {}, the {}: the board is {}.", self.hand_no, serde_json::to_value(street).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(), cards_name(&self.board)), data: json!({"board": self.board}) });
        // Fewer than two can still bet: the rest of the board comes with no more betting.
        if acting.len() < 2 {
            self.to_act = None;
            for s in &mut self.seats {
                s.acted = true;
            }
            events.extend(self.settle());
            return events;
        }
        self.to_act = self.next(self.button, true);
        events.extend(self.your_turn());
        events
    }

    /// The pots as they stand mid-round (journal#1030): what is matched, and apart the bet no one
    /// has called yet, the excess of the one who put in most this round over the next.
    pub fn matched_pots(&self) -> (Vec<Pot>, Option<(usize, u64)>) {
        let mut by_round: Vec<usize> = (0..self.seats.len()).filter(|i| !self.seats[*i].out).collect();
        by_round.sort_by_key(|i| std::cmp::Reverse(self.seats[*i].round));
        let uncalled = match by_round.as_slice() {
            [top, next, ..] if self.seats[*top].live() && self.seats[*top].round > self.seats[*next].round => Some((*top, self.seats[*top].round - self.seats[*next].round)),
            _ => None,
        };
        let put: Vec<u64> = (0..self.seats.len()).map(|i| self.seats[i].hand - uncalled.filter(|(s, _)| *s == i).map_or(0, |(_, e)| e)).collect();
        // A side pot forms only at a live all-in player's cap (journal#1041); the last is uncapped.
        let mut caps: Vec<u64> = (0..self.seats.len()).filter(|i| self.seats[*i].live() && self.seats[*i].all_in).map(|i| put[i]).collect();
        caps.sort_unstable();
        caps.dedup();
        caps.push(u64::MAX);
        let mut pots: Vec<Pot> = Vec::new();
        let mut below = 0;
        for cap in caps {
            let amount: u64 = put.iter().map(|h| (*h).min(cap) - (*h).min(below)).sum();
            let eligible: Vec<usize> = (0..self.seats.len()).filter(|i| self.seats[*i].live() && !(self.seats[*i].all_in && put[*i] < cap)).collect();
            if amount > 0 {
                pots.push((amount, eligible));
            }
            below = cap;
        }
        (pots, uncalled)
    }

    /// The pots, main and side, each with who may win it, from what everyone put in.
    pub fn pots(&self) -> Vec<Pot> {
        let mut levels: Vec<u64> = self.seats.iter().filter(|s| s.hand > 0).map(|s| s.hand).collect();
        levels.sort_unstable();
        levels.dedup();
        let mut pots: Vec<(u64, Vec<usize>)> = Vec::new();
        let mut below = 0;
        for level in levels {
            let amount: u64 = self.seats.iter().map(|s| s.hand.min(level) - s.hand.min(below)).sum();
            let eligible: Vec<usize> = (0..self.seats.len()).filter(|i| self.seats[*i].live() && self.seats[*i].hand >= level).collect();
            match (eligible.is_empty(), pots.last_mut()) {
                // Only folded players put in this much: it goes to the pot below.
                (true, Some(last)) => last.0 += amount,
                (true, None) => pots.push((amount, (0..self.seats.len()).filter(|i| self.seats[*i].live()).collect())),
                (false, Some(last)) if last.1 == eligible => last.0 += amount,
                (false, _) => pots.push((amount, eligible)),
            }
            below = level;
        }
        pots
    }

    fn showdown(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        let mut winners_all: Vec<Value> = Vec::new();
        let mut shown = Vec::new();
        // What each shows, for a program too: the dashboard turns these cards up (CON-152).
        let mut shown_data = Vec::new();
        for i in 0..self.seats.len() {
            if self.seats[i].live() {
                let mut cards = self.seats[i].hole.clone();
                cards.extend(&self.board);
                let rank = best(&cards);
                shown.push(format!("{} shows {} ({})", self.seats[i].id, cards_name(&self.seats[i].hole), rank.name()));
                shown_data.push(json!({"player": self.seats[i].id, "cards": cards_name(&self.seats[i].hole), "cards_list": self.seats[i].hole, "hand": rank.name()}));
            }
        }
        for (amount, eligible) in self.pots() {
            let ranks: Vec<(usize, HandRank)> = eligible.iter().map(|&i| {
                let mut cards = self.seats[i].hole.clone();
                cards.extend(&self.board);
                (i, best(&cards))
            }).collect();
            let top = ranks.iter().map(|(_, r)| r.clone()).max().expect("a pot has someone to win it");
            let mut winners: Vec<usize> = ranks.iter().filter(|(_, r)| *r == top).map(|(i, _)| *i).collect();
            // The odd chip goes to the first winner after the button.
            winners.sort_by_key(|i| (*i + self.seats.len() - self.button - 1) % self.seats.len());
            let share = amount / winners.len() as u64;
            let odd = amount % winners.len() as u64;
            for (k, w) in winners.iter().enumerate() {
                let won = share + if (k as u64) < odd { 1 } else { 0 };
                self.seats[*w].stack += won;
                winners_all.push(json!({"player": self.seats[*w].id, "won": won, "with": top.name()}));
            }
        }
        let said: Vec<String> = winners_all.iter().map(|w| format!("{} wins {} with {}", w["player"].as_str().unwrap_or_default(), w["won"], w["with"].as_str().unwrap_or_default())).collect();
        events.push(Event { to: None, kind: "hand_ended".into(), text: format!("Hand {}, the showdown: the board is {}; {}. {}.", self.hand_no, cards_name(&self.board), shown.join("; "), said.join("; ")), data: json!({"hand": self.hand_no, "board": self.board, "winners": winners_all, "shown": shown_data}) });
        events.extend(self.end_hand());
        events
    }

    fn end_hand(&mut self) -> Vec<Event> {
        self.to_act = None;
        for s in &mut self.seats {
            if s.stack == 0 {
                s.out = true;
            }
        }
        let active = self.active();
        if active.len() < 2 || self.hand_no >= self.rules.max_hands {
            self.over = true;
            let mut standing: Vec<&Seat> = self.seats.iter().collect();
            standing.sort_by_key(|s| std::cmp::Reverse(s.stack));
            let words: Vec<String> = standing.iter().enumerate().map(|(i, s)| format!("{}. {} ({})", i + 1, s.id, s.stack)).collect();
            return vec![Event { to: None, kind: "game_over".into(), text: format!("The game ended after {} hands: {}.", self.hand_no, words.join(", ")), data: json!({"standings": standing.iter().map(|s| json!({"player": s.id, "stack": s.stack})).collect::<Vec<_>>()}) }];
        }
        self.button = self.next(self.button, false).expect("two players at least");
        self.start_hand()
    }

    fn seat_of(&self, player: &str) -> Option<usize> {
        self.seats.iter().position(|s| s.id == player)
    }
}

impl Game for Holdem {
    fn name(&self) -> &str {
        "texas_holdem"
    }

    fn to_act(&self) -> Option<PlayerId> {
        self.to_act.map(|s| self.seats[s].id.clone())
    }

    fn act(&mut self, player: &str, action: &Value) -> Result<Vec<Event>, String> {
        if self.over {
            return Err("the game is over".to_string());
        }
        let seat = self.seat_of(player).ok_or_else(|| format!("{player} is not at this table"))?;
        if self.to_act != Some(seat) {
            return Err(format!("it is not your turn: it is {}'s", self.to_act().unwrap_or_else(|| "nobody's".into())));
        }
        let what = action["do"].as_str().unwrap_or_default();
        let to_call = self.to_call(seat);
        let said = match what {
            "fold" => {
                self.seats[seat].folded = true;
                "folds".to_string()
            }
            "check" => {
                if to_call > 0 {
                    return Err(format!("you cannot check: it is {to_call} to call"));
                }
                "checks".to_string()
            }
            "call" => {
                if to_call == 0 {
                    return Err("there is nothing to call: check, or bet".to_string());
                }
                let paid = self.pay(seat, to_call);
                format!("calls {paid}{}", if self.seats[seat].all_in { ", all in" } else { "" })
            }
            "bet" | "raise" | "all_in" => {
                let s = &self.seats[seat];
                let most = s.round + s.stack;
                let to = if what == "all_in" { most } else { action["amount"].as_u64().ok_or("a bet or raise needs `amount`: the total you bet in this round")? };
                if to > most {
                    return Err(format!("you have {} in all: a bet to {to} is more", most));
                }
                if to <= self.current_bet {
                    if what == "all_in" {
                        let paid = self.pay(seat, to_call);
                        self.seats[seat].acted = true;
                        let mut events = vec![self.acted(seat, &format!("calls {paid}, all in"))];
                        events.extend(self.settle());
                        return Ok(events);
                    }
                    return Err(format!("a bet or raise must be more than the current bet of {}", self.current_bet));
                }
                let raise_by = to - self.current_bet;
                let full = raise_by >= self.min_raise;
                if !full && to < most {
                    return Err(format!("a raise must be to at least {} (the last raise was {})", self.current_bet + self.min_raise, self.min_raise));
                }
                self.pay(seat, to - self.seats[seat].round);
                if full {
                    self.min_raise = raise_by;
                    for (i, other) in self.seats.iter_mut().enumerate() {
                        if i != seat {
                            other.acted = false;
                        }
                    }
                }
                self.current_bet = to;
                // Into an unopened round it is a bet; over a bet, or the blinds, a raise.
                let verb = if to_call == 0 && self.street != Street::Preflop { "bets" } else { "raises to" };
                format!("{verb} {to}{}", if self.seats[seat].all_in { ", all in" } else { "" })
            }
            other => return Err(format!("no action {other:?}: fold, check, call, bet, raise or all_in")),
        };
        self.seats[seat].acted = true;
        let mut events = vec![self.acted(seat, &said)];
        events.extend(self.settle());
        Ok(events)
    }

    fn timeout(&mut self) -> Vec<Event> {
        let Some(seat) = self.to_act else { return Vec::new() };
        let player = self.seats[seat].id.clone();
        let action = if self.to_call(seat) == 0 { json!({"do": "check"}) } else { json!({"do": "fold"}) };
        self.act(&player, &action).unwrap_or_default()
    }

    /// The hand now (CON-150): what everyone may see, and under `you` what only `player` may.
    fn view(&self, player: Option<&str>) -> Value {
        let me = player.and_then(|p| self.seat_of(p));
        let dealt = self.hand_no > 0;
        let role = |i: usize| -> Vec<&str> {
            let mut roles = Vec::new();
            if dealt && !self.seats[i].out {
                if i == self.button {
                    roles.push("button");
                }
                if i == self.sb {
                    roles.push("small_blind");
                }
                if i == self.bb {
                    roles.push("big_blind");
                }
            }
            roles
        };
        let name = |i: usize| self.seats[i].id.clone();
        let (matched, uncalled) = self.matched_pots();
        json!({
            "game": "texas_holdem",
            "hand": self.hand_no,
            "hands_played": self.hand_no.saturating_sub(if self.over { 0 } else { 1 }),
            "max_hands": self.rules.max_hands,
            "blinds": {"small": self.rules.small_blind, "big": self.rules.big_blind},
            "street": self.street,
            "board": cards_name(&self.board),
            "board_cards": self.board,
            "pot": self.pot(),
            "pots": matched.into_iter().map(|(amount, who)| json!({"amount": amount, "players": who.into_iter().map(name).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "uncalled": uncalled.map(|(seat, amount)| json!({"player": name(seat), "amount": amount})),
            "current_bet": self.current_bet,
            "button": dealt.then(|| name(self.button)),
            "small_blind": dealt.then(|| name(self.sb)),
            "big_blind": dealt.then(|| name(self.bb)),
            "to_act": self.to_act(),
            "still_in_hand": self.seats.iter().filter(|s| s.live()).count(),
            "still_in_game": self.seats.iter().filter(|s| !s.out).count(),
            "over": self.over,
            "players": self.seats.iter().enumerate().map(|(i, s)| json!({
                "player": s.id, "stack": s.stack, "bet": s.round, "in_hand": s.hand,
                "folded": s.folded, "all_in": s.all_in, "out": s.out, "roles": role(i),
            })).collect::<Vec<_>>(),
            "you": me.map(|i| json!({
                "player": self.seats[i].id,
                "cards": cards_name(&self.seats[i].hole),
                "cards_list": self.seats[i].hole,
                "stack": self.seats[i].stack,
                "bet": self.seats[i].round,
                "in_hand": self.seats[i].hand,
                "to_call": self.to_call(i),
                "legal": if self.to_act == Some(i) { self.legal(i) } else { Vec::new() },
                "your_turn": self.to_act == Some(i),
                "min_raise_to": self.current_bet + self.min_raise,
                "max_to": self.seats[i].stack + self.seats[i].round,
                "position": role(i),
                "folded": self.seats[i].folded,
            })),
        })
    }

    fn over(&self) -> bool {
        self.over
    }
}

impl Holdem {
    fn acted(&self, seat: usize, said: &str) -> Event {
        Event { to: None, kind: "acted".into(), text: format!("Hand {}: {} {said}.", self.hand_no, self.seats[seat].id), data: json!({"player": self.seats[seat].id, "action": said}) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card by its name: "As", "Td", "2c".
    fn c(name: &str) -> Card {
        let rank = RANKS.iter().position(|r| *r as char == name.chars().next().unwrap()).unwrap() as u8;
        let suit = "shdc".find(name.chars().nth(1).unwrap()).unwrap() as u8;
        rank * 4 + suit
    }
    fn hand(names: &str) -> Vec<Card> {
        names.split_whitespace().map(c).collect()
    }
    fn rank(names: &str) -> HandRank {
        best(&hand(names))
    }

    #[test]
    fn hands_rank_as_poker_does() {
        let order = [
            "As Ks Qs Js Ts",  // royal flush
            "9h 8h 7h 6h 5h",  // straight flush
            "5d 4d 3d 2d Ad",  // the wheel, as a straight flush
            "Kc Kd Kh Ks 2c",  // four of a kind
            "Qc Qd Qh 2s 2c",  // full house
            "Ah 9h 7h 4h 2h",  // flush
            "Tc 9d 8h 7s 6c",  // straight
            "5c 4d 3h 2s Ac",  // the wheel
            "7c 7d 7h Ks 2c",  // trips
            "Jc Jd 4h 4s Ac",  // two pair, ace kicker
            "Jc Jd 4h 4s 9c",  // two pair, nine kicker
            "Ac Ad 9h 6s 3c",  // a pair of aces
            "Kc Kd Ah 6s 3c",  // a pair of kings
            "Ac Qd 9h 6s 3c",  // high card
            "Ac Qd 9h 6s 2c",  // a lower last kicker
        ];
        let ranks: Vec<HandRank> = order.iter().map(|h| rank(h)).collect();
        for pair in ranks.windows(2) {
            assert!(pair[0] > pair[1], "{pair:?}");
        }
        assert_eq!(rank("As Ks Qs Js Ts").name(), "a straight flush");
        assert_eq!(rank("5c 4d 3h 2s Ac").1, vec![3], "the wheel is five high");
        // Best five of seven: the board plays, a split.
        assert_eq!(rank("2c 3d Ah Kh Qh Jh Th"), rank("4c 5d Ah Kh Qh Jh Th"));
        assert_eq!(rank("2c 2d 2h 7s 7c 9d 9h").name(), "a full house", "the best full house of two");
        assert_eq!(rank("2c 2d 2h 7s 7c 9d 9h").1[..2], [0, 7]);
    }

    fn players(n: usize) -> Vec<PlayerId> {
        (0..n).map(|i| format!("p{i}")).collect()
    }
    fn act(t: &mut Holdem, p: &str, a: Value) -> Vec<Event> {
        t.act(p, &a).unwrap_or_else(|e| panic!("{p} {a}: {e}"))
    }

    /// CON-150: the state of the hand tells who is still in, the board, the pots, the roles and
    /// what each has put in; `you` tells the asker its own, and no one else's cards.
    #[test]
    fn the_view_is_the_whole_state_of_the_hand() {
        let (mut t, _) = Holdem::new(&players(3), 7, Rules::default());
        let v = t.view(None);
        // The button p0, p1 small, p2 big, p0 to act; the blinds are in.
        assert_eq!((v["button"].as_str(), v["small_blind"].as_str(), v["big_blind"].as_str()), (Some("p0"), Some("p1"), Some("p2")));
        assert_eq!(v["players"][2]["roles"], json!(["big_blind"]));
        assert_eq!((v["pot"].as_u64(), v["still_in_hand"].as_u64(), v["still_in_game"].as_u64()), (Some(30), Some(3), Some(3)));
        assert_eq!((v["hand"].as_u64(), v["hands_played"].as_u64(), v["max_hands"].as_u64()), (Some(1), Some(0), Some(50)));
        assert_eq!(v["blinds"], json!({"small": 10, "big": 20}));
        // journal#1030: the pots hold what is matched; BB's extra 10 is a bet not called yet.
        assert_eq!(v["pots"], json!([{"amount": 20, "players": ["p0", "p1", "p2"]}]), "what is in, matched: {v}");
        assert_eq!(v["uncalled"], json!({"player": "p2", "amount": 10}));
        act(&mut t, "p0", json!({"do": "raise", "amount": 60}));
        act(&mut t, "p1", json!({"do": "call"}));
        act(&mut t, "p2", json!({"do": "fold"}));
        let v = t.view(None);
        assert_eq!((v["still_in_hand"].as_u64(), v["still_in_game"].as_u64()), (Some(2), Some(3)));
        assert_eq!(v["street"], "flop");
        assert_eq!(v["board_cards"].as_array().unwrap().len(), 3);
        let in_hand: Vec<u64> = v["players"].as_array().unwrap().iter().map(|p| p["in_hand"].as_u64().unwrap()).collect();
        assert_eq!(in_hand, [60, 60, 20], "what each has put in this hand");
        assert_eq!(v["pot"].as_u64(), Some(140));
        let pots: u64 = v["pots"].as_array().unwrap().iter().map(|p| p["amount"].as_u64().unwrap()).sum();
        assert_eq!(pots, 140);
        assert_eq!(v["pots"][0]["players"], json!(["p0", "p1"]), "a folded player can win nothing");
        // After the flop p1, the small blind, acts first.
        let me = t.view(Some("p1"));
        let you = &me["you"];
        assert_eq!((you["in_hand"].as_u64(), you["bet"].as_u64(), you["stack"].as_u64(), you["to_call"].as_u64()), (Some(60), Some(0), Some(940), Some(0)));
        assert_eq!((you["your_turn"].as_bool(), you["max_to"].as_u64()), (Some(true), Some(940)));
        assert_eq!(you["position"], json!(["small_blind"]));
        assert_eq!(you["cards_list"].as_array().unwrap().len(), 2);
        for other in ["p0", "p2"] {
            let theirs = t.view(Some(other))["you"]["cards"].as_str().unwrap().to_string();
            assert!(!me.to_string().contains(&theirs), "{other}'s cards {theirs} in p1's view");
        }
    }

    #[test]
    fn illegal_actions_are_refused_saying_why() {
        let (mut t, events) = Holdem::new(&players(3), 7, Rules::default());
        assert!(events.iter().any(|e| e.kind == "cards" && e.to.as_deref() == Some("p0")));
        // Three players: the button p0, p1 small, p2 big; p0 acts first.
        assert_eq!(t.to_act().as_deref(), Some("p0"));
        assert!(t.act("p1", &json!({"do": "call"})).unwrap_err().contains("not your turn"));
        assert!(t.act("p0", &json!({"do": "check"})).unwrap_err().contains("20 to call"));
        assert!(t.act("p0", &json!({"do": "raise", "amount": 30})).unwrap_err().contains("at least 40"));
        assert!(t.act("p0", &json!({"do": "raise", "amount": 5000})).unwrap_err().contains("1000 in all"));
        assert!(t.act("p0", &json!({"do": "dance"})).unwrap_err().contains("no action"));
        // What each sees: their own cards only.
        let view = t.view(Some("p1"));
        assert_ne!(view["you"]["cards"], t.view(Some("p2"))["you"]["cards"]);
        assert!(t.view(None)["you"].is_null());
    }

    #[test]
    fn everyone_folds_to_the_big_blind() {
        let (mut t, _) = Holdem::new(&players(3), 1, Rules::default());
        act(&mut t, "p0", json!({"do": "fold"}));
        let events = act(&mut t, "p1", json!({"do": "fold"}));
        assert!(events.iter().any(|e| e.kind == "hand_ended" && e.text.contains("p2 wins 30")), "{events:?}");
        let view = t.view(None);
        assert_eq!(view["hand"], 2, "the next hand is dealt");
        // In it p2 is the small blind: what it holds and what it has posted make its 1010.
        assert_eq!(view["players"][2]["stack"].as_u64().unwrap() + view["players"][2]["bet"].as_u64().unwrap(), 1010);
    }

    /// journal#1041: mid-round, a side pot forms only at a short all-in's cap; bets that differ
    /// only because the round goes on are one pot.
    #[test]
    fn side_pots_form_only_at_an_all_in() {
        let (mut t, _) = Holdem::new(&players(3), 5, Rules::default());
        // p1 has only 50 in all; p0 raises to 100, p1 goes all in for 50, p2 (big blind) to act.
        t.seats[1].stack = 50 - t.seats[1].hand;
        act(&mut t, "p0", json!({"do": "raise", "amount": 100}));
        let v = t.view(None);
        assert_eq!(v["pots"].as_array().unwrap().len(), 1, "no side pot without an all-in: {v}");
        act(&mut t, "p1", json!({"do": "all_in"}));
        let v = t.view(None);
        // In: p0 100, p1 50 all in, p2 20. p0's 50 above the next bet is not called yet; the rest,
        // all under p1's cap, is one pot all three can win.
        assert_eq!(v["uncalled"], json!({"player": "p0", "amount": 50}), "{v}");
        assert_eq!(v["pots"], json!([{"amount": 120, "players": ["p0", "p1", "p2"]}]), "{v}");
        act(&mut t, "p2", json!({"do": "call"}));
        let v = t.view(None);
        assert_eq!(v["pots"], json!([{"amount": 150, "players": ["p0", "p1", "p2"]}, {"amount": 100, "players": ["p0", "p2"]}]), "a side pot at p1's cap: {v}");
    }

    /// Three players all in with different stacks: a main pot and a side pot, won by different hands.
    #[test]
    fn side_pots_go_to_the_best_hand_each_can_win() {
        let rules = Rules { stack: 1000, small_blind: 10, big_blind: 20, max_hands: 1 };
        // Deal from the seat after the button (p0): p1, p2, p0, then again; then flop, turn, river.
        // p1: As Ah, p2: Ks Kh, p0: Qs Qh. Board: 2c 7d 9h 3s 4c: aces win everything they can.
        let deck = hand("As Ks Qs Ah Kh Qh 2c 7d 9h 3s 4c");
        let (mut t, _) = Holdem::with_deck(&players(3), 3, rules, deck);
        // Unequal stacks: p1 has 100, p2 has 300, p0 has 1000.
        t.seats[1].stack = 100 - t.seats[1].hand;
        t.seats[2].stack = 300 - t.seats[2].hand;
        let events = act(&mut t, "p0", json!({"do": "all_in"}));
        assert!(events.iter().any(|e| e.text.contains("p0 raises to 1000")), "{events:?}");
        act(&mut t, "p1", json!({"do": "all_in"}));
        let events = act(&mut t, "p2", json!({"do": "all_in"}));
        let ended = events.iter().find(|e| e.kind == "hand_ended").expect("a showdown");
        // Main pot 300 (100 x 3) to p1's aces; side pot 400 (200 x 2) to p2's kings; p0's extra 700 back.
        let won: Vec<(String, u64)> = ended.data["winners"].as_array().unwrap().iter().map(|w| (w["player"].as_str().unwrap().to_string(), w["won"].as_u64().unwrap())).collect();
        assert_eq!(won, [("p1".to_string(), 300), ("p2".to_string(), 400), ("p0".to_string(), 700)]);
        // CON-152: the shown cards are in the data too, for the dashboard to turn up.
        let shown = ended.data["shown"].as_array().unwrap();
        assert_eq!(shown.len(), 3, "{shown:?}");
        assert!(shown.iter().all(|s| s["cards_list"].as_array().unwrap().len() == 2 && s["hand"].is_string() && s["player"].is_string()));
        assert!(t.over(), "one hand only");
        let over = events.iter().find(|e| e.kind == "game_over").unwrap();
        assert!(over.text.contains("1. p0 (700)") && over.text.contains("2. p2 (400)") && over.text.contains("3. p1 (300)"), "{}", over.text);
    }

    #[test]
    fn a_split_pot_shares_and_the_odd_chip_goes_after_the_button() {
        let rules = Rules { stack: 1000, small_blind: 5, big_blind: 10, max_hands: 1 };
        // Heads-up: the button p0 is the small blind; dealt p1, p0, p1, p0. Board plays a royal flush.
        let deck = hand("2c 3d 4c 5d Ah Kh Qh Jh Th");
        let (mut t, _) = Holdem::with_deck(&players(2), 9, rules, deck);
        assert_eq!(t.to_act().as_deref(), Some("p0"), "heads-up, the small blind acts first preflop");
        act(&mut t, "p0", json!({"do": "call"}));
        act(&mut t, "p1", json!({"do": "check"}));
        for _ in 0..3 {
            act(&mut t, "p1", json!({"do": "check"}));
            act(&mut t, "p0", json!({"do": "check"}));
        }
        let view = t.view(None);
        assert_eq!((view["players"][0]["stack"].as_u64(), view["players"][1]["stack"].as_u64()), (Some(1000), Some(1000)), "a 20 pot, split");
    }

    #[test]
    fn a_silent_player_checks_if_it_can_and_folds_if_not() {
        let (mut t, _) = Holdem::new(&players(2), 4, Rules::default());
        let events = t.timeout();
        assert!(events.iter().any(|e| e.text.contains("p0 folds")), "facing the big blind it folds: {events:?}");
        let mut t = Holdem::new(&players(2), 4, Rules::default()).0;
        act(&mut t, "p0", json!({"do": "call"}));
        let events = t.timeout();
        assert!(events.iter().any(|e| e.text.contains("p1 checks")), "{events:?}");
    }

    /// The same seed and the same actions give the same game.
    #[test]
    fn a_game_is_its_seed_and_its_actions() {
        let play = || {
            let (mut t, mut all) = Holdem::new(&players(4), 42, Rules { max_hands: 5, ..Rules::default() });
            let mut guard = 0;
            while !t.over() && guard < 500 {
                guard += 1;
                let p = t.to_act().unwrap();
                let view = t.view(Some(&p));
                let legal: Vec<&str> = view["you"]["legal"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
                let a = if legal.contains(&"check") { json!({"do": "check"}) } else { json!({"do": "call"}) };
                all.extend(t.act(&p, &a).unwrap());
            }
            assert!(t.over(), "five hands end the game");
            let chips: u64 = t.view(None)["players"].as_array().unwrap().iter().map(|p| p["stack"].as_u64().unwrap()).sum();
            assert_eq!(chips, 4000, "no chip made or lost");
            all.iter().map(|e| e.text.clone()).collect::<Vec<_>>()
        };
        assert_eq!(play(), play());
    }
}
