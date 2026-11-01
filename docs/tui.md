# Terminal UI

The terminal frontend (IF-11) is built design-first against [design/concept](design/concept/README.md). The agent loop is not wired in yet: the TUI runs on named view states, and the composer publishes every kind of transcript item through debug commands, so each component can be looked at live.

## Run

```bash
./run.sh                        # build (release) and open the TUI at the splash
./run.sh preview main-session   # open on a named state (see `screen --list`)
./run.sh --graphics off         # text only; auto|sixel|kitty|iterm2|off
./run.sh --debug                # debug build
```

`run.ps1` does the same on Windows (`-Graphics`, `-DebugBuild`). The scripts set the environment the installer will set globally later: `MULTITOOL_GRAPHICS` (default `auto`) and `COLORTERM=truecolor`.

**Text first.** The design targets text mode: every picture has a text form drawn in the same cells — the logo as half-block pixel art, icons as glyphs, mind maps as box-drawing cards and arrows. Terminals with a graphics protocol (kitty, WezTerm, Ghostty; sixel where enabled) get pictures instead, made from SVG for the cells they cover. A protocol forced with `--graphics` is used only when the terminal answers a graphics query; others keep the text forms.

## Flow

splash (OK / Enter) → startup page (recent sessions, quick actions) → session.

## Keys

| Where | Key | Does |
|---|---|---|
| splash, startup | Esc | quit |
| startup | ↑ ↓ · Tab / ← → · Enter · `n` `r` `s` `t` `?` | move · switch panel · choose · quick action |
| session | Enter | send the composer line (text → your message; `/…` → a command) |
| session | `/exit`, `/quit`, Ctrl+C | quit |
| session | Esc | close the notification toast |
| session | PageUp / PageDown · Home / End · mouse wheel | scroll the transcript · to the start / back to the newest |
| session | Ctrl+T · Ctrl+W · Alt+1..9 · click a tab or `+` | new session tab · close the tab · switch tab |
| session | click `⚙ Settings` | open the settings window |
| permission prompt | 1 · 2 · 3 · ←→ / Tab · Enter · Esc · click a button | allow once · always allow · deny · choose · confirm · deny |
| settings | Tab · ↑↓ · Space / Enter · Ctrl+S · Esc · click | change pane · move · select (provider, field, box) · save · close |
| maps | Ctrl+click a `#N` ref (mind map) · click a node or an arrow pill with refs (flowchart) · click again or the ticket | unfold the journal entry as a ticket beside the map (below it when the map needs the width) · fold it |
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
| `/mindmap-new` · `/flowchart-new` | a mind map / a flowchart created (empty) |
| `/mindmap` | a mind map with every node kind and status, journal refs, a cross link, flowchart links |
| `/flowchart` · `/flowchart down` | every flowchart symbol and logic gate · a flow drawn top to bottom |
| `/permission` · `/settings` | the permission prompt · the settings window |
| `/tool read\|edit\|cat\|tg\|a2a\|journal\|fail\|run` | a tool call: plain row, diff, numbered output, Telegram send, agent send, journal write, failure, running |
| `/hook start\|compact\|block\|fail\|timeout\|run` · `/hooks` | hooks firing on a loop event (session start and compaction run `fact-check-cognitive-protocol.sh` and `harness-lookup.sh`) · every hook outcome |
| `/toast [text]` · `/tab [name]` · `/clear` | a toast · a new session tab · an empty transcript |
| `/exit`, `/quit` | quit |

Tool calls, journal and mind-map events attach to the newest assistant turn when it is the last item, as they do in a real session.

## Screens as images

Every mockup has a named state that renders through the same `draw` as the live TUI (`--compare` puts the mockup beside it); a few states show components that have no mockup:

| State | Shows | Mockup |
|---|---|---|
| `splash` | logo, name, version, slogan, welcome text, OK | `splash-*` |
| `startup` | recent sessions, quick actions, status line, tip | `startup-*` |
| `welcome` | a fresh session: welcome card, quick tips | `welcome-*` |
| `main-session` | turns, tool rows, an expanded diff, status line, composer | `main-session-*` |
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

## Layout rules

Settled with the project owner while matching the mockups; the journal holds each with its reason.

- Two rows separate transcript cards, split in quarter rows (▂ ▆): three quarters of a row of padding under one card, a half-row gap of page colour, three quarters of a row above the next. The stripe is a whole cell wide and ends the same way, so it is exactly as tall as its card. (A half-cell stripe could not end with a quarter-row gap: a cell holds two colours.)
- Tool cards: only a file edit's diff has a background; tool output, notes and sent messages are plain text under the tool's name.
- The startup page selection is an accent outline without a fill, shown only in the focused panel.
- The one-row header shows the logo only when the terminal draws pictures.
- Cards end straight at the transcript's edge: corners cut with quadrant blocks read as glitches in a real terminal; only the stripes end round.
- Voice messages show their waveform as spaced bars on one baseline.

## Maps

Two separate tools ([ADR 0011](decisions/0011-mind-maps-and-flowcharts.md)): a mind map maps what is known, a flowchart shows how something flows. Their events are MINDMAP and FLOWCHART cards. A **mind map** is a tree from its root: topic ◆, goal ◎, decision ◈, fact ●, hypothesis ◐, question ?, task ☐/☑, idea ✦, dead end ✕, note ✎; a node can be done (muted) or dropped (struck through), point at journal entries (#137), and cross links (dashed, labelled) join branches; a node, a branch or a link can point at flowcharts (`▸ flow #3`). A **flowchart** is a block diagram by the canon: start/end, process, decision, input/output, subprocess, document, data store, manual input, preparation, connector, delay, note, and the logic gates AND, OR, NOT, NAND, NOR, XOR, XNOR (IEC symbols, `o` = inverted output). A flowchart runs left to right or top to bottom (its direction); a node lists its journal refs on its last line, an arrow on its pill ("no #131"). Clicking a ref unfolds the entry as a ticket inside the map card. The node changed by an event is tinted with the accent. One layout draws both forms: filled cards and box-drawing lines in text, SVG in graphics terminals.

## Hooks

A HOOK card shows the loop event the hooks ran on (SessionStart, PostCompact, PreToolUse with the call it saw, PostToolUse, Stop, UserPromptSubmit, ...) and one row per script: ✓ ok, ✓ with the lines it added to the context, ⊘ blocked with the reason (a veto, IF-6), ✗ failed with the exit code and stderr tail, ◷ timed out, ◌ running, and how long it took. The frame takes the colour of the worst outcome: warning for a veto or a timeout, error for a failure.
