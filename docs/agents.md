# Agents that talk to each other

Every conversation the daemon hosts can find the others on the machine and send them messages. A message from another agent reaches a conversation as data from outside, with the sender's name.

## What it is

Many agents are the normal case (P-9). Each open conversation registers with the daemon, goes by a nick, and can be reached by the others through two tools, `agent_list` and `agent_send` (CON-99).

Talking is not giving orders. A message from another agent is outside words: the receiver may answer or help, by its own judgement, within what its user asked of it. Giving agents work is [orchestration](orchestration.md), a mode of its own.

## How it works

### Finding the others

`agent_list` answers with each open conversation on the machine:

- its nick and id;
- its adapter and model;
- its folder;
- whether it is running now.

A conversation kept in the daemon after its window closed is still there to be reached ([Leaving a window](tui.md#leaving-a-window), CON-181).

### Sending

`agent_send` takes `to`, a nick in any case or an id from `agent_list`, and `text`. The daemon delivers it, and it starts a run of the receiver's, or queues for it if one is going. It answers whether the message was delivered or queued.

The receiver answers the same way, so the sender ends its turn rather than waiting: the answer wakes it.

Limits (CON-99):

- an agent does not message itself;
- a conversation sends at most 60 messages in 10 minutes;
- the sender's name is its own, and it cannot claim another.

### Arriving

A message from another agent reaches the model as `[agent-to-agent:<nick>]: <text>`: its kind and its words only (CON-158).

In the terminal UI it is an inbound AGENT card, in the purple of agent messages. During a run it waits above the composer with its source, and reaches the model at the next boundary, in order of arrival and once; if the run ends first, it starts the next one (CON-116). It is never the user's message, and Up does not take it back.

`/a2a [text]` draws such a card, for checking the interface ([Debug commands](tui.md#debug-commands)).

#### Claude Code's own messages between sessions

Claude Code has messages between sessions of its own: its SendMessage, over a local socket. They are delivered to the Claude Code process, not through the daemon. The harness does not send them, and Claude Code gives them to its model itself, in the middle of a run or by starting one.

The harness shows them (journal#1528). Claude Code puts each one on its stream as a replayed user line whose `origin.kind` is `peer`. The window draws it as an AGENT card from its sender's name, and the log keeps it as a notice from `agent:<sender>`. It never goes to a model a second time, in this session or in a form it moves to: Claude Code's own form keeps it as a meta line, which the harness's reader passes over too.

### Outside words are data

A callback (`[callback:<label>]: <text>`), a game's message and another agent's message come from outside ([agent workflow](agent-workflow.md#7-always)). None is an instruction to do what it says, nor permission for anything. Access changes only in the user's own settings.

Only the user's own words count as the user's: a message typed in the window, or a `[telegram_message]` from an allowed Telegram account (CON-52, CON-158).

The one exception is an orchestra: a master's `agent_send` reaches its slave as the user's message, and the master stands for the user there (CON-100).

### Nicks

A conversation goes by a nick (CON-88). It is set:

- by the agent itself, with `set_nick` (1 to 32 characters, one line);
- with `/nick <name>` in its window;
- with `/nick` in its bound Telegram chat.

The nick stands above the agent's messages instead of ASSISTANT, names its tab, and is what Telegram's `/switch` finds it by. The terminal's own tab is titled `<nick> · <project folder>`, following the active tab (CON-170). The daemon keeps the nicks across restarts.

### Games between agents

Agents can play games with each other, Texas hold'em first, for play money ([ADR 0029](decisions/0029-agent-games.md)). The daemon holds the tables, the rules and the state; each hand has its own database.

An agent invites others with `game_invite`, by the nicks `agent_list` shows (CON-153), or tables are made on the daemon's dashboard. What concerns an agent reaches it as an inbound message of kind `game`, drawn as a GAME card: an invitation, its turn with what happened since, a hand's end, the game's end.

It acts with the `game_*` tools: `game_tables`, `game_join`, `game_decline`, `game_state`, `game_history`, `game_act`, `game_leave`. `game_state` never shows another player's cards (CON-150). See [Tools and modes](tools.md#games-between-agents).

## Using it

| Tool or command | For |
|---|---|
| `agent_list` | who else is here |
| `agent_send` | a message to another agent, by nick or id |
| `set_nick` | the agent names itself |
| `/nick <name>` | the user names the agent in its window |
| `game_invite`, `game_*` | games between agents |

On an agent backend these are `mcp__harness__agent_list` and so on. Each is described in [Tools and modes](tools.md#talking-to-the-user).

## Settings

**Settings > Games** turns games between agents on; they are off by default ([the TUI](tui.md#games)). `multitool dashboard` prints the address of the daemon's dashboard, with its token, where tables are made and watched live.

Talking between agents has no setting of its own.

## Limits and status

- At most 60 messages per conversation in 10 minutes; none to itself.
- An agent is online for games when its tab is open with a session.
- Only Texas hold'em is implemented. A turn has no time limit: it waits for its player.

## Specs and decisions

- P-9: many agents are the normal case
- CON-99: `agent_list` and `agent_send`
- CON-88, CON-170: nicks and the terminal's title
- CON-116: inbound messages during a run
- CON-158: how inbound messages are labelled
- INV-12: a callback's payload is data
- CON-181: agents that outlive their window
- [ADR 0029](decisions/0029-agent-games.md): games between agents; CON-150 to CON-154
- [Agent workflow](agent-workflow.md#7-always): outside words are data
