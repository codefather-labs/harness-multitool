# Terminal UI

The terminal frontend (IF-11) is built design-first against [design/concept](design/concept/README.md). A new session is real: a plain message goes to the agent loop, and what the loop logs is what the transcript draws (`frontend-tui/src/live.rs`). The other named states are design fixtures. In any session the composer's `/` commands publish every kind of transcript item, so each component can be looked at live. The startup page lists this folder's real sessions; opening one draws its log and the next message continues it.

## Run

```bash
./run.sh                        # build (release) and open the TUI at the splash
./run.sh preview main-session   # open on a named state (see `screen --list`)
./run.sh --graphics off         # text only; auto|sixel|kitty|iterm2|off
./run.sh --debug                # debug build
```

`run.ps1` does the same on Windows (`-Graphics`, `-DebugBuild`). The scripts set the environment the installer will set globally later: `MULTITOOL_GRAPHICS` (default `auto`) and `COLORTERM=truecolor`.

**Text first.** Text mode is the primary target: the owner works in Ptyxis (VTE), which draws no pictures, so every screen must read well without them. The design targets text mode: every picture has a text form drawn in the same cells — the logo as half-block pixel art, icons as glyphs, mind maps as box-drawing cards and arrows. Terminals with a graphics protocol (kitty, WezTerm, Ghostty; sixel where enabled) get pictures instead, made from SVG for the cells they cover. A protocol forced with `--graphics` is used only when the terminal answers a graphics query; others keep the text forms.

## A session

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

Keys come from the environment: `ANTHROPIC_API_KEY`, or `<PROVIDER>_API_KEY` for a provider of that name (`[session.auth.<name>]` can name another variable with `env`, or hold the key as `key`). Ctrl+S in the settings window saves the provider and the model for new sessions; a running session keeps its model. The agent also has the journal, mind-map and flowchart tools; their results are JOURNAL, MINDMAP and FLOWCHART cards, and writes to memory need no prompt. Each repository has one journal, in the OS's data directory under `journals/`; every checkout shares it. `/reload`, or the agent's `plugins_reload` tool, rebuilds the local build and swaps the binary in the same terminal once nothing is running: every tab comes back with its transcript and draft, and sessions go on. The daemon is replaced too; it keeps its port and windows, so callback URLs stay valid, though callbacks sent during the second of the swap fail. Unfolded blocks, open tickets, and cards that are not in a session's log (callback journal and flow cards, debug items) do not come back. The system prompt carries the project's specifications and constraints from the journal and the titles of its maps. When a conversation passes `compact_at_tokens` (70% of `context_window` by default), older messages stop being sent, with a note of what was asked and which tools ran, and a COMPACTED card shows; the log keeps all of it, `PostCompact` hooks run. Each tab is its own session, with its own log in the OS's data directory. Reads inside the project run; writes, commands and network use ask (`1` once, `2` always for this session, `3` deny). Esc stops a generating agent at its next chunk, and the next message continues the conversation.

Hooks are shell commands, run on the loop's events; each run is a HOOK card:

```toml
[[session.hooks]]
event = "PreToolUse"            # SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, PostCompact; none means all
command = "./check.sh"
name = "check.sh"               # what the card shows
```

The command sees `HOOK_EVENT`, `HOOK_CALL` (a tool event's call as JSON) and `HOOK_PROMPT`. Exit 0: its output lines join the agent's context. Exit 2: a veto, with stderr as the reason. Any other exit: failed, and it blocks nothing.

Callbacks reach a tab through the daemon, which the UI starts when needed (`multitool daemon`; it listens on localhost and goes on after the UI closes). Each tab has a URL, which its agent knows. POST JSON with a unique `uuid` and a `type`:

| `type` | Fields | What happens |
|---|---|---|
| `notification` | `text`, `label` | a CALLBACK card, and the agent's next input, marked as external data |
| `journal` | `text`, `category`, `tags` | a journal entry and its card; no agent turn |
| `flowstate_update` | `map`, `node`, `instance` (or `correlation_id`), `state` | the chart drawn with that node active; no agent turn. With one instance, a new card per event; with several, one row each, updated in place, opened by a click or Ctrl+G, pulsing softly for a second on an update |

A project's `.harness/conditions.toml` adds types of its own. Such a callback is a signal: it is logged and reaches no one until a condition turns true, and then the agent gets one message for the whole batch, with the condition, the policy files and the trigger log's path.

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

A repeated `uuid` is handled once. A type with no handler gets 400 and reaches no one. Everything is logged in `daemon/triggers.jsonl` in the OS's data directory. The settings' Callbacks tab shows the standard types, this project's conditions and the daemon; it writes `.harness/callbacks.toml` and makes the daemon read it (`r` alone reloads). Custom handlers are crates implementing `CallbackHandler`, listed in `crates/cli/src/plugins.rs` of a local build: each serves its own types, can be chosen for a standard type in the tab, and can pass a message to the agent. The file is read when a window opens; other kinds of conditions are crates implementing `Condition`, not yet selectable.

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
| anywhere | F2 | next theme (dark, light, nord, monokai, cyan) |

## Debug commands

Plain text in the composer becomes a USER turn. Commands publish the other items (`/help` lists them in the transcript):

| Command | Publishes |
|---|---|
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
| `/exit`, `/quit`, `/q` | quit |
| `/help`, `/?` | the list of commands |

Short aliases: `/a` (agent), `/telegram` (tg), `/agent-msg` (a2a), `/cb` (callback), `/map-new` (flowchart-new), `/mind` (mindmap), `/flow` and `/map` (flowchart), `/t` (tool).

Tool calls, journal and mind-map events attach to the newest assistant turn when it is the last item, as they do in a real session.

## Debugging sessions

- `multitool trace` lists the scenarios of the agent loop; `multitool trace <name>` prints one's log, one line per event.
- `multitool screen --scenario <name> --png out.png` draws the transcript that scenario leaves.
- `multitool preview welcome --script <name>` runs a scenario on test doubles in the live UI.
- `multitool run "<message>" [--yes]` sends one message through a real session with no UI and prints its log; `--yes` answers permission prompts with yes.
- `MULTITOOL_WIRE_LOG=<file>` writes every request body and response chunk exchanged with a model to that file, never the headers, so the keys stay out of it. A model's reasoning, where the provider streams it, shows inside its turn above the answer, live and then folded to one line, and as `thinking` lines in `multitool run`.
- `tools/pty_session.py <absolute path to multitool>` drives the real UI in a pty against a fake provider endpoint: real HTTP, tools and permission prompts. It prints the screen at each step and the idle CPU. It needs the Python package `pyte`.

## Screens as images

Every mockup has a named state that renders through the same `draw` as the live TUI (`--compare` puts the mockup beside it); a few states show components that have no mockup:

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

Two separate tools ([ADR 0011](decisions/0011-mind-maps-and-flowcharts.md)): a mind map maps what is known, a flowchart shows how something flows. Their events are MINDMAP and FLOWCHART cards. A **mind map** is a tree from its root: topic ◆, goal ◎, decision ◈, fact ●, hypothesis ◐, question ?, task ☐/☑, idea ✦, dead end ✕, note ✎; a node can be done (muted) or dropped (struck through), point at journal entries (#137), and cross links (dashed, labelled) join branches; a node, a branch or a link can point at flowcharts (`▸ flow #3`). A **flowchart** is a block diagram by the canon: start/end, process, decision, input/output, subprocess, document, data store, manual input, preparation, connector, delay, note, and the logic gates AND, OR, NOT, NAND, NOR, XOR, XNOR (IEC symbols, `o` = inverted output). A flowchart runs left to right or top to bottom (its direction); a node lists its journal refs on its last line, an arrow on its pill ("no #131"). Clicking a ref unfolds the entry as a ticket inside the map card; only one ticket is open in a transcript, and in a ticket of several entries only one is unfolded. The node a live run is at (ADR 0012) is outlined in the accent, like the ticket. The node changed by an event is tinted with the accent. One layout draws both forms: filled cards and box-drawing lines in text, SVG in graphics terminals.

## Hooks

A HOOK card shows the loop event the hooks ran on (SessionStart, PostCompact, PreToolUse with the call it saw, PostToolUse, Stop, UserPromptSubmit, ...) and one row per script: ✓ ok, ✓ with the lines it added to the context, ⊘ blocked with the reason (a veto, IF-6), ✗ failed with the exit code and stderr tail, ◷ timed out, ◌ running, and how long it took. The frame takes the colour of the worst outcome: warning for a veto or a timeout, error for a failure.
