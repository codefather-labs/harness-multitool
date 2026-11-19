# Terminal UI

This page covers the terminal frontend: how to run it, its profiles, keys, sessions, settings, the daemon, Telegram, callbacks, and the commands and screens for debugging it.

## Overview

The terminal frontend (IF-11) is built design-first against [design/concept](design/concept/README.md).

A new session is real. A plain message goes to the agent loop, and what the loop logs is what the transcript draws (`frontend-tui/src/live.rs`). The other named states are design fixtures.

In any session the composer's `/` commands publish every kind of transcript item, so each component can be looked at live.

The startup page lists this folder's real sessions. Opening one draws its log, and the next message continues it.

## Run

```bash
./install.sh --local             # build it, copy it, put `multitool` on your PATH, run the daemon as a user
                                # service, and install Claude Code's rules and self-check hook (CON-162)
./install.sh --local --no-service --no-agent-setup   # only the binary
./install.sh --uninstall        # remove the binary, the service and the agent setup; keep data and backups
.\install.ps1 -Local            # Windows: the same, the daemon as the Scheduled Task multitool-daemon at logon,
                                # the hook as harness-selfcheck-reminder.ps1; -NoService, -NoAgentSetup, -Uninstall
multitool                       # from any folder: the start page, on the profile last chosen with Use in the settings
multitool --profile work        # on another profile for this run only, as autostart wants it (CON-127)
multitool --dir ~/p --new         # a new session tab in that folder at once; --resume <id> opens that conversation (CON-167)
multitool chat dump [id|name]   # a conversation, its Claude Code form and the project's journal with its maps, into multitool_dumps/ here (CON-171); named with the time of the dump, -2, -3… if taken; progress on stderr (CON-172)
multitool chat load <file.zip>  # into this folder's project: new ids every time (CON-175), paths moved, the journal once; `chat list` lists
multitool daemon restart        # stop the daemon, its runs included, and start this build's on the same port (CON-128),
                                # through its user service when one is installed (CON-162)
multitool daemon stop           # stop it and start none; --pid N only if it is that process (the service's ExecStop)
systemd-cgtop multitool.slice   # each delegate (claude, codex) runs in a scope of its own under multitool-agents.slice (CON-166);
                                # `multitool daemon --status` gives the daemon's own rss_mb and peak_mb
./run.sh                        # build (release) from this checkout and open the TUI at the splash
./run.sh preview main-session   # open on a named state (see `screen --list`)
./run.sh --graphics off         # text only; auto|sixel|kitty|iterm2|off
./run.sh --debug                # debug build
```

`run.ps1` does the same on Windows (`-Graphics`, `-DebugBuild`).

### The installed binary

The installed `multitool` is a copy: run the installer again to update it.

On Linux and macOS it is linked from `~/.local/bin`. If that folder is not on your PATH, the installer offers to add one marked line to your shell's profile, and `--uninstall` takes that line back. The profile file is:

- zsh: `~/.zshrc`;
- bash: `~/.bashrc`, or `~/.bash_profile` on macOS;
- fish: `~/.config/fish/conf.d/harness-multitool.fish`;
- other shells: `~/.profile`.

On Windows the installer puts its folder on your user PATH. It says if another `multitool` comes first there.

It is a link, not a shell alias. An alias works only in an interactive shell of one kind. A command on PATH works from any shell, script or launcher.

### Graphics

Graphics default to `auto` without `MULTITOOL_GRAPHICS`. Nothing in the harness reads `COLORTERM`.

**Text first.** Text mode is the primary target. The owner works in Ptyxis (VTE), which draws no pictures, so every screen must read well without them.

The design targets text mode: every picture has a text form drawn in the same cells. The logo is half-block pixel art, icons are glyphs, and mind maps are box-drawing cards and arrows.

Terminals with a graphics protocol (kitty, WezTerm, Ghostty; sixel where enabled) get pictures instead, made from SVG for the cells they cover. A protocol forced with `--graphics` is used only when the terminal answers a graphics query; others keep the text forms.

## Profiles

A profile is a separate set of the harness's settings and sign-ins ([ADR 0026](decisions/0026-profiles.md)). It holds its provider, model and keys, and its own Claude Code sign-in, ChatGPT sign-in and Codex sign-in. Each lives in a folder of its own under `~/.harness/profiles/<name>/` (`profile.toml`, `claude/`, `chatgpt/`, `codex/`).

Conversations are not the profile's. They belong to the daemon, and every profile sees the same ones (CON-126). Moving a window to another profile moves its tabs too (CON-127).

What the user set up in Claude Code and Codex reaches a delegate on every profile (CON-145):

- plugins, skills, agents, commands, hooks, rules and instructions are linked into the profile's folders;
- settings are shared without their sign-in keys;
- the user's MCP servers are passed along.

A sign-in is never copied: each profile signs in for itself.

- **Settings > Profiles**: the list, with Add, Edit, Delete and Use. Enter on a line is Use: the window and the next start run on it. The card under the cursor shows its folder and accounts (CON-125).

- **Settings > Provider**: signs the current profile in or out. Claude Code is signed in only by the unmodified `claude auth login` in the profile's own folder; the harness never reads, stores or passes on a Claude.ai token (CON-123, CON-56). The line shows the account's email and plan.

- From the command line: `multitool profile list`, `multitool profile add <name>` (a clean profile: defaults, no key, no sign-in), `multitool login claude-code|codex|chatgpt` and `multitool logout ...` for the current profile, and `--profile <name>` for one run only, as autostart wants it.

- `default` runs Claude Code on the user's own folder until its own is signed in (CON-120, CON-122).

## Flow

splash (the key points of the harness as bullets, ideas and concepts tagged; OK / Enter) → startup page (recent sessions, quick actions) → session. On a terminal too short for the splash, the logo is dropped and the list stays whole.

## Keys

| Where | Key | Does |
|---|---|---|
| splash, startup | Esc | quit |
| startup | ↑ ↓ · Tab / ← → · Enter · `n` `r` `s` `t` `?` | move · switch panel · choose · quick action |
| session | Enter | send the composer line (text → your message; `/…` → a command) |
| session | `/exit`, `/quit`, `/q`, Ctrl+C | quit |
| session | Esc | stop a generating agent, else close the notification toast |
| session | PageUp / PageDown · Home / End · mouse wheel | scroll the transcript · to the start / back to the newest |
| session | Ctrl+G · click a FLOW row | open or close the newest flowchart instance · that one |
| session | Ctrl+O · click `▸ Thought for …` | fold or unfold the newest reasoning · that one |
| session | Ctrl+T · Ctrl+W · Alt+1..9 · click a tab or `+` | new session tab · close the tab · switch tab |
| session | click `⚙ Settings` | open the settings window |
| permission prompt | 1 · 2 · 3 · ←→ / Tab · Enter · Esc · click a button | allow once · always allow · deny · choose · confirm · deny |
| settings | Tab · ↑↓ · Space / Enter · Ctrl+S · Esc · click | change pane · move · select (provider, field, box) · save · close |
| maps | Ctrl+click a `#N` ref (mind map) · click a node or an arrow pill with refs (flowchart) · click an entry's header in the ticket · click again or the ticket | unfold the entries as a ticket on the right of the map card · show that entry, fold the others · fold the ticket |
| maps | press and drag on a map | pan a map wider than its room (`◀ ▶` mark the hidden parts; no zoom) |
| anywhere | F2 | next theme (dark, light, paper, sand, blossom, mist, nord, monokai, cyan; CON-180) |

## A session

Each tab is its own session, with its own log in the OS's data directory.

### Provider, model and keys

Choose the provider and the model in the configuration (`config.toml` in the OS's config directory, see `multitool setup`):

```toml
[session]
provider = "anthropic"          # the default; or a name under [session.providers]
model = "claude-opus-5-5"       # the default
max_cost_usd = 5.0              # optional budget; max_turns too

[session.providers.local]       # any OpenAI-compatible endpoint; this one is Ollama
kind = "openai"
base_url = "http://127.0.0.1:11434/v1"
context_window = 32000

[session.auth.local]
none = true                     # no key needed
```

Keys come from the environment: `ANTHROPIC_API_KEY`, or `<PROVIDER>_API_KEY` for a provider of that name. `[session.auth.<name>]` can name another variable with `env`, or hold the key as `key`.

Ctrl+S in the settings window saves the provider and the model for new sessions. A running session keeps its model.

Claude subscriptions cannot be used this way, by Anthropic's terms: Anthropic models take an API key. `multitool login chatgpt` and `logout` sign in to ChatGPT from the command line, as the Provider page does (see Settings > Provider below).

### Permissions

Reads inside the project run. Writes, commands and network use ask:

- `1` once;
- `2` always for this session;
- `3` deny.

### Sending messages during a run

- A message sent during a run waits above the composer. It reaches the agent at its next step, without interrupting it.
- `↑` on an empty composer takes the last one back.
- `Esc` stops the run, and then what is queued goes at once. Esc stops a generating agent at its next chunk, and the next message continues the conversation.

When the agent asks a question, its options wait above the composer. `↑`/`↓` and `Enter`, or a number, pick one. Typing answers in your own words.

### Journal, maps and memory

The agent also has the journal, mind-map and flowchart tools. Their results are JOURNAL, MINDMAP and FLOWCHART cards, and writes to memory need no prompt.

Each repository has one journal, in the OS's data directory under `journals/`. Every checkout shares it.

The system prompt carries the project's specifications and constraints from the journal and the titles of its maps.

### Planning first

The agent is told to plan before it builds ([ADR 0021](decisions/0021-agent-behaviour.md)). `session.plan_first` says what happens if it writes with no plan made: `remind`, `enforce` or `off`.

`/plan` puts the session in plan mode. The agent reads, searches and writes to its journal and maps, and nothing in the project can change. The agent can also enter it when asked to plan. It leaves only when you approve its plan, or with `/plan off`.

### Tasks

`/task <command>` makes the next message a task. It is done only when the command passes, then every check in the project's `.harness/acceptance.toml` (`[[check]] spec = N`, `command = "..."`).

- A failing check goes back to the model with its evidence.
- Each verification goes into the journal, and the evidence lists the specifications nothing checks.
- `/task` alone uses only the file; `/task off` cancels.

The file runs only as you approved it. The first task, and any task after it changes, lists its commands and waits for `/task trust` (`--yes` on the command line).

A finished task ends the tab's session. `multitool run --verify <command>` or `--task` does the same from the command line.

### Step-debug mode

`/stepdebug` turns on step-debug mode ([ADR 0013](decisions/0013-step-debug-mode.md)) for large pipelines. The agent walks a running pipeline step by step through a debug router it builds by one contract (`multitool debug contract`). `multitool debug check <url>` tests the router.

Each finished cycle is saved to `.harness/debug/runs.sqlite`; `multitool debug runs` lists them.

### Compaction

When a conversation passes `compact_at_tokens` (70% of `context_window` by default), older messages stop being sent. A note says what was asked and which tools ran, and a COMPACTED card shows. The log keeps all of it, and `PostCompact` hooks run.

### Naming the agent

`/nick <name>` names the agent, and the agent can do it itself with `set_nick`. The name stands above its messages instead of ASSISTANT, each session its own.

The terminal's own tab is named after it too, `<nick> · <project folder>`, following the active tab (CON-170).

### Setting up a project

`/init` or `multitool init` creates `docs/` and `.harness/`, each file with an example, overwriting nothing. A project with neither is asked once.

In an initialized project:

- the agent is told which file to update after each write to its journal or maps;
- it gets one reminder at the end of a turn if the documentation and the journal disagree;
- `multitool docs check` runs the same check by hand or in CI. It looks at ids and statuses, never at the prose.

### Claude Code and Codex as delegates

`./run.sh --claude-code` builds with Claude Code as a delegate ([ADR 0020](decisions/0020-claude-code-delegate.md), off in the default build).

- The `claude-code` row runs the unmodified `claude`, signed in by itself, and shows what it does. It always runs with `--chrome`, so its browser tools are there in every tab and every agent (CON-133).
- A `codex` row runs the unmodified Codex CLI the same way. The Provider page lists Codex where it is installed. It is signed in by `multitool login codex` or the page's button, in the profile's own Codex folder (CON-137). It does not see your own hooks or servers, and for now it can only read.

### The status line

While a run lasts, the right of the status line says what the agent is doing and for how long:

- waiting for the model;
- thinking;
- writing;
- running a tool;
- waiting for your answer.

### Hook commands

Hooks are shell commands, run on the loop's events; each run is a HOOK card (see Hooks below):

```toml
[[session.hooks]]
event = "PreToolUse"            # SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, PostCompact; none means all
command = "./check.sh"
name = "check.sh"               # what the card shows
```

The command sees `HOOK_EVENT`, `HOOK_CALL` (a tool event's call as JSON) and `HOOK_PROMPT`. Its exit code decides the outcome:

- exit 0: its output lines join the agent's context;
- exit 2: a veto, with stderr as the reason;
- any other exit: failed, and it blocks nothing.

## Settings

The settings window has a page per topic. Ctrl+S saves; Profiles is described under Profiles above.

### Provider

The Provider page chooses the model's provider, which also decides how it signs in. The button under the list runs that sign-in.

- For `gpt-subscription`, Login opens OpenAI's page and waits, with Cancel and then Log out. Signing in puts the plan to use at once.
- For an API provider, the button takes a key, masked, saved in the configuration readable only by you.
- `claude-subscription` is listed and only answers that Anthropic's terms forbid it.

The model row opens a dropdown of what the provider's API lists, each with its price per 1M tokens where known. On a plan its tokens are included, and the API price is shown for comparison.

Ctrl+S saves the choice, and the open sessions go on under it with their history.

### Memory

The Memory page sets `map_max_nodes` (CON-114): the most nodes a mind map or flowchart holds before an add is refused. It is 200 by default, and 0 means no limit.

A session already running keeps the limit it started with until it restarts. Each MCP server reads the configuration as it starts.

### Journal

Settings > Journal sends the window's project's journal todos to its GitHub issues through the daemon (ADR 0028):

- a todo opens an issue;
- what is linked to it comments;
- closing it closes the issue.

It is per project, off until turned on. gh runs with the project's own folder (or one named there, such as an account's) and its own `gh auth login` (c). r checks gh off the window's thread. A public repository asks once.

`multitool journal issues` does the same from the command line.

### Chats

Settings > Chats lists the window's project's conversations by adapter (CON-171).

A click on one (or Enter) shows a [ Dump ] button under it. It dumps the conversation into the project's multitool_dumps/ (named with the time, never over a file) with a progress toast (CON-172).

[ Load… ] is always there. It opens the system's file picker for a .zip: zenity or kdialog on Linux, the Finder on macOS, the Windows dialog. It loads the archive with its toast, always under new ids, so the same archive twice is two conversations (CON-175).

With no picker or no graphical session, the page says why. It lists the archives of multitool_dumps/ and the project's root, with a path to type (CON-173).

Keys: d dump, l load. A loaded copy shows under 'native' until it is opened.

### Behaviour

Settings > Behaviour holds 'On exit' and 'On an accidental loss'. See Leaving a window below.

### Games

Settings > Games turns games between agents on (off by default): Texas hold'em, play money.

Tables are made on the daemon's dashboard (`multitool dashboard` prints its address with its token), which invites the agents online.

An agent is told by a GAME card and acts with the `game_*` tools. A GAME card comes when it is invited, at its turn (with what happened since), at a hand's end and at the game's end.

### Callbacks

The settings' Callbacks tab is described under Callbacks below, in The Callbacks tab.

## Reload

`/reload`, or the agent's `plugins_reload` tool, rebuilds the local build and swaps the binary in the same terminal once nothing is running. A window on the installed `multitool` builds nothing and restarts on the binary installed now (CON-129).

Every tab comes back with its transcript and draft, and sessions go on.

The daemon is replaced too. It keeps its port and windows, so callback URLs stay valid, though callbacks sent during the second of the swap fail.

These do not come back:

- unfolded blocks;
- open tickets;
- cards that are not in a session's log (callback journal and flow cards, debug items).

## The daemon

Callbacks reach a tab through the daemon, which the UI starts when needed (`multitool daemon`). It listens on localhost and goes on after the UI closes.

### Connection

Windows, session servers and the command line talk to it over its local socket, `<data dir>/daemon/daemon.sock` (ADR 0027, CON-130). There is one connection per process, and the daemon pushes each window's events on it. HTTP serves only callbacks and `/health`.

Windows of different builds share one daemon as long as they speak its protocol.

A daemon of another protocol that runs other windows' conversations is waited for, never worked around. The window asks it every 2 seconds to stop if it is idle, then starts its own.

`MULTITOOL_CLIENT_DEBUG=1` logs each connection, loss and welcome to stderr.

### When the connection drops

When the connection drops, the window tries again every 2 seconds for as long as it is open. It starts a daemon if none comes back. It shows a toast at every loss and every connection made (CON-131).

While it is down, a second toast stays up with a pulsing bar and the attempts so far, naming the reason. What you send meanwhile is refused at once with a toast, and a typed message stays in the composer.

The window never hangs on the daemon (CON-132). With no connection a call fails at once, and a daemon that does not answer in 2 seconds counts as lost.

### Copying a conversation

While the daemon makes or brings up to date a conversation's copy for another adapter or profile, or reads one in, that conversation shows a toast with a progress bar (CON-136, CON-144). The toast shows:

- what is being done;
- messages (or MB) done out of the total;
- the share.

Only that tab is held. What you send into it is refused with a toast and stays in the composer. Every other tab and the settings go on as usual.

### The terminal's tab title

The terminal's tab is named '<agent's nick> · <project folder>' (CON-170). While an agent of the window works or a background task of its runs, a moon spins before it, 🌑 to 🌘 (CON-174).

## Telegram

With a bot token in `MULTITOOL_TELEGRAM_TOKEN` and `[telegram] allowed_users` in the configuration, the daemon carries a Telegram channel.

A message from an allowed user reaches the window that listened last as a Telegram card, marked as data for the agent. The agent answers with `telegram_send`, asking permission first.

Use a bot of its own for this: another program polling the same bot competes for its messages.

### Commands

From the chat (CON-167):

- `/open` lists the conversations the daemon has hosted, newest first, with a button each. An open one is switched to. A closed one opens in a new terminal window in its project, where its chat or topic reaches it again. `[telegram] terminal` names the command, else xdg-terminal-exec, ptyxis, gnome-terminal or x-terminal-emulator (Linux for now).
- `/new` opens a new session in a chosen project.
- `/reload` reloads every open window.
- `/restart` restarts the daemon through its service after a ✅.

Each says when it is done: the daemon back with its new pid, the windows back. The daemon keeps that register in `daemon/known.json`.

### The Service button

A `🛠 Service` button stays at the bottom of each chat (CON-169). It opens these controls as buttons, with no command to type: Open, New, Reload windows, Restart daemon, Sessions and Which session. `/service` brings it back.

### Messages to closed sessions

A message to a chat whose session is closed waits only until that session is open again. Then it goes by itself with the time it was sent.

`multitool daemon --status` shows the counters.

## Leaving a window

Leaving a window (CON-181): /exit, and Ctrl+C pressed twice when no run of the tab is going, ask what this window's agents do. The first press on a running run still stops it. The choices are:

1. stop now;
2. let the command under way finish;
3. let them finish their work (they go on until they stop by themselves, then end);
4. keep them running in the daemon, where Telegram, callbacks and other agents reach them.

'Remember my choice' makes it Settings > Behaviour 'On exit', which then leaves without asking.

A window lost without a word (killed, its terminal or ssh dropped, silent for 30 s) does 'On an accidental loss', by default 3.

With no window, a question an agent asks waits in its conversation for the next window that opens it. A window with no daemon can only stop.

## Callbacks

Callbacks reach a tab through the daemon (see The daemon above).

### Callback URLs

Each tab has a URL, and each session has one of its own that never changes (CON-164). An outside app should use the session's one.

- Its agent knows it from its system prompt and the `callback_url` tool (with how to use it).
- The agent checks it with `callback_check`: a test sent to it, and what it has seen (CON-165).
- A session not open keeps what comes until it opens.

### Standard types

POST JSON with a unique `uuid` and a `type`:

| `type` | Fields | What happens |
|---|---|---|
| `notification` | `text`, `label` | a CALLBACK card, and the agent's next input, marked as external data |
| `journal` | `text`, `category`, `tags` | a journal entry and its card; no agent turn |
| `flowstate_update` | `map`, `node`, `instance` (or `correlation_id`), `state` | the chart drawn with that node active; no agent turn. With one instance, a new card per event; with several, one row each, updated in place, opened by a click or Ctrl+G, pulsing softly for a second on an update |

### Conditions

A project's `.harness/conditions.toml` adds types of its own. Such a callback is a signal: it is logged and reaches no one until a condition turns true. Then the agent gets one message for the whole batch, with the condition, the policy files and the trigger log's path.

```toml
[[condition]]
name = "pipeline keeps failing"
on = "iteration_failed"          # the callback type it listens to
at_least = 3                    # how many signals make it true (1 by default)
within_s = 300                  # only signals this recent (all by default)
cooldown_s = 60                 # after a message, none for this long; signals still go to the log
field = "ms"                    # optional: count only signals where the field passes
op = "gt"                       # eq, ne, gt, lt, ge, le
value = 1000
policies = [".harness/policies/pipeline.md"]
```

### Delivery and the log

- A repeated `uuid` is handled once.
- A type with no handler gets 400 and reaches no one.
- Everything is logged in `daemon/triggers.jsonl` in the OS's data directory.

### The Callbacks tab

The settings' Callbacks tab shows the standard types, this project's conditions and the daemon. It writes `.harness/callbacks.toml` and makes the daemon read it (`r` alone reloads). The file is read when a window opens.

Custom handlers are crates implementing `CallbackHandler`, listed in `crates/cli/src/plugins.rs` of a local build. Each serves its own types, can be chosen for a standard type in the tab, and can pass a message to the agent.

Other kinds of conditions are crates implementing `Condition`, not yet selectable.

### The project's own callbacks

Under "This conversation's callbacks" the tab lists the project's own callbacks, `.harness/callbacks/<name>/` (CON-183, [callbacks.md](callbacks.md)), for the active tab's conversation. Each shows its types and its state:

- not approved;
- approved;
- changed: approve again;
- cannot run, with why.

Keys:

- `Enter` (or `Space`, or a click) turns one on or off at once (not with Ctrl+S). On approves its handler as it is now when it needs it, pinned by its SHA-256, since it runs as you, and subscribes this conversation. Off unsubscribes it (journal#1382).
- `c` puts the conversation's callback URL into the composer.

A window opening a project whose handlers wait for approval says so in a toast.

`multitool callback new|build|list` makes, builds and lists them.

## Debug commands

Plain text in the composer becomes a USER turn. Commands publish the other items (`/help` lists them in the transcript):

| Command | Publishes |
|---|---|
| `/version` | the harness package version in a VERSION card (local; no model request) |
| `/agent [text]` | an assistant reply |
| `/busy` | toggles `generating…` on the newest assistant turn |
| `/tg [text]` | a Telegram message and its notification toast |
| `/voice [text]` | a Telegram voice message with its transcript |
| `/a2a [text]` | an agent-to-agent message (with the a2a route when another tab is open) |
| `/callback [text]` | an HTTP callback with its status |
| `/reminder [text]` | a journal reminder |
| `/journal` | a JOURNAL event |
| `/thinking` | a model's reasoning, live |
| `/mindmap-new` · `/flowchart-new` | a mind map / a flowchart created (empty) |
| `/mindmap` | a mind map with every node kind and status, journal refs, a cross link, flowchart links |
| `/flowchart` · `/flowchart down` | every flowchart symbol and logic gate · a flow drawn top to bottom |
| `/permission` · `/settings` | the permission prompt · the settings window |
| `/tool read\|edit\|cat\|tg\|a2a\|journal\|fail\|run` | a tool call: plain row, diff, numbered output, Telegram send, agent send, journal write, failure, running |
| `/hook start\|compact\|block\|fail\|timeout\|run` · `/hooks` | hooks firing on a loop event (session start and compaction run `fact-check-cognitive-protocol.sh` and `harness-lookup.sh`) · every hook outcome |
| `/toast [text]` · `/tab [name]` · `/clear` | a toast · a new session tab · an empty transcript |
| `/skills` | the harness's skills here, the project's and the user's (`.harness/skills`, ADR 0030) |
| `/tabs` | a TABS card: one row per open tab, in order, with its title, its own ADAPTER and MODEL (a dash when none is set), the active tab marked (CON-107; local, no model request) |
| `/exit`, `/quit`, `/q` | quit |
| `/help`, `/?` | the list of commands |

Short aliases: `/a` (agent), `/telegram` (tg), `/agent-msg` (a2a), `/cb` (callback), `/map-new` (flowchart-new), `/mind` (mindmap), `/flow` and `/map` (flowchart), `/t` (tool).

Tool calls, journal and mind-map events attach to the newest assistant turn when it is the last item, as they do in a real session.

## Debugging sessions

- `multitool trace` lists the scenarios of the agent loop; `multitool trace <name>` prints one's log, one line per event.

- `multitool screen --scenario <name> --png out.png` draws the transcript that scenario leaves.

- `multitool preview welcome --script <name>` runs a scenario on test doubles in the live UI.

- `multitool run "<message>" [--yes]` sends one message through a real session with no UI and prints its log; `--yes` answers permission prompts with yes.

- `MULTITOOL_WIRE_LOG=<file>` writes every request body and response chunk exchanged with a model to that file, never the headers, so the keys stay out of it.

- A model's reasoning, where the provider streams it, shows inside its turn above the answer, live and then folded to one line, and as `thinking` lines in `multitool run`.

- `tools/pty_session.py <absolute path to multitool>` drives the real UI in a pty against a fake provider endpoint: real HTTP, tools and permission prompts. It prints the screen at each step and the idle CPU. It needs the Python package `pyte`.

## Screens as images

Every mockup has a named state that renders through the same `draw` as the live TUI (`--compare` puts the mockup beside it). A few states show components that have no mockup:

| State | Shows | Mockup |
|---|---|---|
| `splash` | logo, name, version, slogan, welcome text, OK | `splash-*` |
| `startup` | recent sessions, quick actions, status line, tip | `startup-*` |
| `welcome` | a fresh session: welcome card, quick tips | `welcome-*` |
| `main-session` | turns, tool rows, an expanded diff, status line, composer | `main-session-*` |
| `settings-callbacks` | the settings on the Callbacks tab, with a dropdown open | `settings-callbacks-*` |
| `flow-instances` | three passes of one chart: closed rows, one opened, one pulsing | none (designed in code) |
| `reasoning` | a model's reasoning live, folded and unfolded, inside the turn above the answer | `reasoning-*` |
| `inbound-messages` | Telegram, voice, agent-to-agent, callback, reminder; a toast | `inbound-messages-*` |
| `tool-calls` | tool cards: numbered output with a fold, Telegram send, agent send, journal write, failure, running | `tool-calls-*` |
| `journal-mindmap-events` | JOURNAL event, an empty map's creation, a drawn flowchart | `journal-mindmap-events-*` |
| `session-tabs` | four sessions as tabs with busy / unread / attention marks | `session-tabs-*` |
| `maps` | every flowchart symbol and gate, every mind-map node kind | none (designed in code) |
| `hooks` | every hook outcome | none (designed in code) |
| `permission` | a shell command waiting for approval | `permission-*` |
| `settings-overlay` | the settings window on its provider page | `settings-overlay-*` |


```bash
multitool screen --list
multitool screen tool-calls --theme light --png out.png             # as a graphics terminal shows it
multitool screen tool-calls --no-graphics --size 160x45 --png out.png  # text mode, any size
multitool screen startup --compare --png cmp.png                     # side by side with the mockup
multitool screen splash --eyes-closed --png blink.png                # the logo's blink frame
multitool screen maps --no-graphics --size 160x70 --png maps.png    # every map component
multitool screen hooks --no-graphics --size 160x50 --png hooks.png  # every hook outcome
```

Other `screen` options: `--mockups <dir>`, `--font` / `--font-bold` (other faces than the built-in DejaVu Sans Mono), `--px` (font size), `--cell-h` (cell height).

## Layout rules

Settled with the project owner while matching the mockups; the journal holds each with its reason.

- Two rows separate transcript cards, split in quarter rows (▂ ▆): three quarters of a row of padding under one card, a half-row gap of page colour, three quarters of a row above the next. The stripe is a whole cell wide and ends the same way, so it is exactly as tall as its card. (A half-cell stripe could not end with a quarter-row gap: a cell holds two colours.)

- Tool cards: only a file edit's diff has a background; tool output, notes and sent messages are plain text under the tool's name.

- The startup page selection is an accent outline without a fill, shown only in the focused panel.

- The one-row header shows the logo only when the terminal draws pictures.

- Cards end straight at the transcript's edge: corners cut with quadrant blocks read as glitches in a real terminal; only the stripes end round.

- Voice messages show their waveform as spaced bars on one baseline.

## Maps

Two separate tools ([ADR 0011](decisions/0011-mind-maps-and-flowcharts.md)): a mind map maps what is known, a flowchart shows how something flows. Their events are MINDMAP and FLOWCHART cards. One layout draws both forms: filled cards and box-drawing lines in text, SVG in graphics terminals.

### Mind maps

A **mind map** is a tree from its root. Its node kinds are topic ◆, goal ◎, decision ◈, fact ●, hypothesis ◐, question ?, task ☐/☑, idea ✦, dead end ✕, note ✎.

- A node can be done (muted) or dropped (struck through).
- A node can point at journal entries (#137).
- Cross links (dashed, labelled) join branches.
- A node, a branch or a link can point at flowcharts (`▸ flow #3`).

### Flowcharts

A **flowchart** is a block diagram by the canon. Its symbols are start/end, process, decision, input/output, subprocess, document, data store, manual input, preparation, connector, delay and note. Its logic gates are AND, OR, NOT, NAND, NOR, XOR, XNOR (IEC symbols, `o` = inverted output).

A flowchart runs left to right or top to bottom (its direction). A node lists its journal refs on its last line, an arrow on its pill ("no #131").

### Tickets and highlights

Clicking a ref unfolds the entry as a ticket inside the map card. Only one ticket is open in a transcript, and in a ticket of several entries only one is unfolded.

The node a live run is at (ADR 0012) is outlined in the accent, like the ticket. The node changed by an event is tinted with the accent.

## Hooks

A HOOK card shows the loop event the hooks ran on (SessionStart, PostCompact, PreToolUse with the call it saw, PostToolUse, Stop, UserPromptSubmit, ...). It has one row per script, with how long it took:

- ✓ ok;
- ✓ with the lines it added to the context;
- ⊘ blocked with the reason (a veto, IF-6);
- ✗ failed with the exit code and stderr tail;
- ◷ timed out;
- ◌ running.

The frame takes the colour of the worst outcome: warning for a veto or a timeout, error for a failure.

Drawing never takes a session down (CON-113). A card that cannot be drawn shows a line saying so, and its error goes to `draw-errors.log` in the data folder. One item takes at most 4000 rows, and its last row says how many more were not drawn.
