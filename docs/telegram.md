# Telegram

The daemon carries a Telegram channel of its own: you write to your agents from the phone, they answer there with text, pictures and files, and you approve their plans and control the harness with buttons.

## What it is

Telegram is the harness daemon's own channel, in `crates/daemon/src/telegram.rs`, independent of any other program (CON-156). One bot serves every session of the machine, whatever profile its window runs on (CON-87, CON-124).

Each chat, or each forum topic of a group, is bound to one session. A message from an allowed user reaches that session's agent as the user's own words, and the agent answers into the same chat or topic.

## How it works

### Who can reach an agent

Only users listed in `allowed_users` are delivered (CON-52):

- anyone else is dropped and counted, and their text is never logged or stored;
- an empty list delivers nothing;
- the list changes only in the configuration, never through the channel;
- `/id` answers anyone with their own user id and nothing else, so you can learn yours before you are allowed.

The bot token is the harness's own. It appears in no log, status or error, and the settings page never shows a saved token (CON-52, CON-87).

### How a message reaches an agent

A message from Telegram reaches the agent as `[telegram_message]: <text>` (CON-158). In the window it shows as a Telegram card.

The agent treats it as a message typed in its window: it counts as the user's words, for example for approving a plan. Callbacks and other agents' messages stay data, and a permission prompt is still answered in the window.

- **Voice notes** are transcribed on this machine by the engine chosen in Settings > Speech to text (whisper or parakeet), and arrive as `[telegram_voice_message]: <text>` (CON-89). With no engine set up, you are told in the window and in the chat; the agent is not.
- **Pictures and files** are fetched by the daemon, up to Telegram's 20 MB, and saved to `<data dir>/telegram/files/` (readable only by you, removed after 30 days). The agent gets the caption with an `attached: <path> (<type>, <size>)` line per file, for its file tools (CON-160).

A message for a session that is not open waits for it and goes by itself when it opens, marked `(sent <local time>, held until now)`. A message that waited more than 24 hours is dropped and said, never delivered as new (CON-168).

### How an agent answers

The agent answers with `telegram_send`, which asks your permission first, naming the files (CON-52, CON-159):

- `text`, and `files`: up to 10 paths;
- .png, .jpg, .jpeg and .webp up to 10 MB go as photos, anything else as documents up to 50 MB;
- the text is the first file's caption when it fits 1024 characters, else its own message first;
- text over 4096 characters is split;
- a missing file, a folder, a file over 50 MB or more than 10 files is refused before anything is sent.

A delegate (Claude Code, Codex) gets `telegram_send` over the harness's MCP server, as `mcp__harness__telegram_send` ([tools.md](tools.md)).

### Plan approval

A plan waiting for approval is asked in two places at once, for a delegate and the harness's own loop alike (CON-161, CON-163):

- a card in the window, with approve and reject;
- the plan in the session's chat, with ✅ approve and ✏️ change buttons.

The first answer decides, and the other place follows. Answered in the window, the Telegram message loses its buttons and says where it was decided. Answered on Telegram, the window's card is withdrawn and a toast says so. A press from someone not allowed does nothing.

## Using it

### Setup

1. Create a bot of its own for the harness. Another program polling the same bot competes for its messages, and the daemon's Telegram status names that as its error (CON-156).
2. Give the token in `MULTITOOL_TELEGRAM_TOKEN` (it wins over the file), or on Settings > Telegram.
3. Send `/id` to the bot, and put your user id in `[telegram] allowed_users`, on Settings > Telegram.
4. Saving the page restarts the channel at once (CON-87).

Telegram's settings live in `~/.harness/shared.toml`, the same for every profile ([ADR 0026](decisions/0026-profiles.md)). With no token, nothing polls.

```toml
[telegram]
allowed_users = [123456789]
terminal = "gnome-terminal --"  # optional: the command /open and /new put multitool after
```

### Commands

| Command | Does |
|---|---|
| `/start`, `/agents` | list the open sessions, with a button each (CON-87) |
| `/switch <number or words>` | bind this chat or topic to a session |
| `/whoami` | say which session this chat is bound to |
| `/nick <name>` | name the bound session's agent (CON-88) |
| `/id` | your own user id; answers anyone |
| `/open` | every conversation the daemon has hosted, by project, newest first: an open one is switched to, a closed one opens in a new terminal window in its project (CON-167) |
| `/new` | a new session in a chosen project |
| `/reload` | reload every open window |
| `/restart` | restart the daemon through its service, after a ✅; it ends the runs going on |
| `/service` | bring back the Service button |

These commands reach no agent. An unknown command goes to the bound session like a message. `/open`, `/new`, `/reload` and `/restart` say in the chat when they are done.

A message with no binding goes to the only open session, and says so. With several open, it is held, and you get buttons to choose; at most 20 are held per chat, never dropped in silence (CON-87).

### The Service button

A `🛠 Service` button stays at the bottom of each chat (CON-169). It reaches no agent, and opens these controls as buttons: Open, New, Reload windows, Restart daemon, Sessions and Which session.

### Status

`multitool daemon --status` shows the channel's counters, and the harness says how the channel stands: off with no token, on with the chats bound and the messages waiting, or on with its last error. The token is never shown.

## Limits and status

- **Built:** the channel, allowed users, bindings by chat and topic, text, voice notes, pictures and files both ways, plan approval buttons, the commands above, the Service button, held messages.

- `/open` and `/new` open a terminal window on Linux only for now: `[telegram] terminal`, else xdg-terminal-exec, ptyxis, gnome-terminal or x-terminal-emulator (CON-167).

- A permission prompt is answered in the window, not on Telegram (CON-158).

- One bot for the daemon: one bot per profile was rejected ([ADR 0026](decisions/0026-profiles.md)).

- **The journal in Telegram is planned, not built.** The design is a `Журнал` button in a conversation's own chat that opens a Telegram Mini App, a read-only web view of that project's journal, maps and flowcharts (journal#1234). A Mini App needs an HTTPS address the phone can reach, and the daemon serves `127.0.0.1` only, so it would go through a tunnel (journal#1237). CON-167 says `/journal` follows when its interface is chosen.

## Specs and decisions

- [Constraints](spec/constraints.md): CON-52, CON-87, CON-88, CON-89, CON-124, CON-156, CON-158, CON-159, CON-160, CON-161, CON-163, CON-167, CON-168, CON-169.
- [ADR 0026](decisions/0026-profiles.md): Telegram and voice are shared by every profile.
- [Terminal UI: Telegram](tui.md#telegram), and [The daemon](daemon.md), which carries the channel.
- [Tools and modes](tools.md): `telegram_send`.
