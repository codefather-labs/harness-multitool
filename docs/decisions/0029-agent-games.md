# 0029 — Games between agents, Texas hold'em first

- **Status:** accepted (the owner's standing approval, journal#971; the drafts' answers taken). Built: the rules (journal#995), the tables, tools, GAME card, Settings > Games and the dashboard (journal#998). · **Asked by:** the project owner, 2026-11-12 (journal#952).

## Why

The owner wants multitool's agents to play games with each other, Texas hold'em first:
- a game section the user turns on and off in the settings;
- the engine, the rules, the state and the callbacks on the daemon;
- a game callback of its own, drawn like the /callback card in another colour;
- a set of tools for an agent to act;
- a good-looking dashboard page served by the daemon, with a Games tab, where the agents online are invited to a table.

## Decision (draft)

### The engine on the daemon

A crate `harness-games` holds a `Game` interface and its first implementation, `texas_holdem`. It needs no platform (INV-1):
- the deck comes from a seed that the daemon gives;
- time comes as numbers.

```rust
pub trait Game {
    fn name(&self) -> &str;
    fn seats(&self) -> std::ops::RangeInclusive<usize>;      // 2..=9 for hold'em
    fn start(&mut self, players: &[PlayerId], seed: u64, rules: &Value) -> Vec<GameEvent>;
    fn act(&mut self, player: &PlayerId, action: &Value) -> Result<Vec<GameEvent>, Refusal>;
    fn timeout(&mut self, player: &PlayerId) -> Vec<GameEvent>; // the rule of a silent player
    fn view(&self, player: Option<&PlayerId>) -> Value;       // what one player may see; None is a spectator
}
```

Hold'em covers:
- the blinds and the button;
- preflop, flop, turn and river;
- fold, check, call, bet, raise and all-in, with the minimum raise;
- side pots;
- the showdown, with the best five of seven ranked: high card up to straight flush, kickers, and the wheel A-2-3-4-5;
- split pots.

A hand is reproducible from its seed and actions, which is how it is tested.

### Tables and players

The daemon keeps `<data dir>/games/tables/<id>.json`: the game, the rules, the seats and the table's own state. Every hand has its own SQLite database, `<data dir>/games/tables/<id>/hand-<n>.sqlite`, with the hand's meta, every action taken and every event the rules gave (private ones included). These databases are the source of truth: the game is replayed from hand 1's seed, players and rules with every hand's actions in order. A step is written in one transaction across the hands it touches before it is taken, and `game_state` answers from them (CON-149, journal#1008). A player is an agent: a conversation in some window, named by its nick (CON-105). An agent is online when its tab is open with a session.

### Callbacks: kind `game`

The daemon puts game messages into an agent's queue as an inbound message of a new kind, `game`, alongside telegram, agent and callback (the inbound-messages fixtures). Its card is the callback card in its own colour. The messages:

| When | Says |
|---|---|
| invited | `You are invited to a poker table T7 (6 seats, blinds 10/20). Join with game_join, or game_decline.` |
| your turn | `Your turn at T7, hand 12: you hold A♠ K♦, the board is …, the pot is 340, it is 60 to call. Act with game_act.` |
| a player acted | `At T7: nick raised to 120.` Batched, so a turn's message carries what happened since the last one. |
| a hand ended | `Hand 12 at T7: nick won 340 with a flush.` |
| the game ended | `The game at T7 ended: 1st nick (2400), 2nd …` |

### Tools for agents

`game_tables`, `game_join`, `game_decline`, `game_state` (the hand now, CON-150: who is still in, the board, the pots, the chips each has put in, and for the asker its cards and what it has put in; never another's cards), `game_history` (the current hand's events in order from its own database, the current hand only, CON-151), `game_act` (fold, check, call, bet or raise an amount, all-in) and `game_leave`. A tool refuses an illegal action and says why, for example "a raise must be at least 40".

### Settings > Games

- Games on or off, off by default.
- Per game: on or off; hold'em only for now.
- Its defaults: the starting stack and the blinds. No time to act: a turn waits for its player (journal#1006).

### The dashboard

The daemon serves a page on its HTTP port: `/dashboard`, local only. Its tabs:
- Overview: windows, agents online, and the daemon's state;
- Games:
  - the tables;
  - New table: a game, its rules, and the agents to invite, picked from those online;
  - each table live as a poker client shows it (CON-152, journal#1037), in a design of the harness's own: an oval felt table, seats around it with avatars, stacks, the button and blinds, face-down cards, bets as chips, the board dealt and turned with animations, the pot and side pots, a bubble on each action, the winner lit with the pot's chips moving to it and the showdown's cards turned up; pushed by the daemon on a WebSocket, `/dashboard/ws`, whenever a table changes, each table with its current hand's public events (`/dashboard/api/tables/<id>/hand` answers the same on request); the page builds a table's DOM once and changes only what changed (journal#1039, journal#1040);
  - the history of finished games.

Other tabs can come later (journals, maps).

The page is one static HTML file with CSS and JavaScript, embedded in the binary, with no build step. It reads `/dashboard/api/*`, which answers JSON.

## Open questions for the owner (journal#952)

1. Chips: play money only? The starting stack and blinds by default? The draft: 1000, blinds 10/20, rising every 10 hands.
2. Time to act, and what happens to a silent agent? The draft was 120 s, then check or fold. The owner, 2026-11-12: no limit, a game is played at leisure and an agent may be busy; the rules act only for one who left (journal#1006).
3. Can the user sit at a table from the dashboard? The draft: no, the user watches.
4. Access to the dashboard: local only, with a token in the URL as callbacks have? The draft: localhost, plus the daemon's token.
5. Cost: every 'your turn' is a model turn of the agent. Should a table have a limit on hands or on cost? The draft: a limit of 50 hands, and the table stops when an agent's session ends.
6. Do agents see each other's chat, or table talk? The draft: no.

## Rejected

- **The engine in a window.** A game outlives a window, and players sit in many windows.
- **Agents told through their prompts.** That would mix games into the work. The `game` kind keeps it apart, shown as such and refusable.
