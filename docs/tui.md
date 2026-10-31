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
| `/map-new` · `/map` | a map created (empty) · a flowchart updated, drawn |
| `/flow` · `/mind` | every flowchart symbol and logic gate · every mind-map node kind and status |
| `/tool read\|edit\|cat\|tg\|a2a\|journal\|fail\|run` | a tool call: plain row, diff, numbered output, Telegram send, agent send, journal write, failure, running |
| `/hook start\|compact\|block\|fail\|timeout\|run` · `/hooks` | hooks firing on a loop event (session start and compaction run `fact-check-cognitive-protocol.sh` and `harness-lookup.sh`) · every hook outcome |
| `/toast [text]` · `/tab [name]` · `/clear` | a toast · a new session tab · an empty transcript |
| `/exit`, `/quit` | quit |

Tool calls, journal and mind-map events attach to the newest assistant turn when it is the last item, as they do in a real session.

## Screens as images

Every mockup has a named state that renders through the same `draw` as the live TUI:

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

- Between two transcript cards is one row: its upper half pads the card above, its lower half the card below, and a thin line in the page colour separates them; card corners are cut with quadrant blocks so they read as rounded.
- Tool cards: only a file edit's diff has a background; tool output, notes and sent messages are plain text under the tool's name.
- The startup page selection is an accent outline without a fill, shown only in the focused panel.
- The one-row header shows the logo only when the terminal draws pictures.
- Cards end straight at the transcript's edge (cut corners read as glitches there); only the stripes end round.

## Maps

Two kinds, as in the journal's mind maps. A **mind map** is a tree from its root: topic ◆, goal ◎, decision ◈, fact ●, hypothesis ◐, question ?, task ☐/☑, idea ✦, dead end ✕, note ✎; a node can be done (muted) or dropped (struck through), point at journal entries (#137), and cross links (dashed, labelled) join branches. A **flowchart** is a block diagram by the canon: start/end, process, decision, input/output, subprocess, document, data store, manual input, preparation, connector, delay, note, and the logic gates AND, OR, NOT, NAND, NOR, XOR, XNOR (IEC symbols, `o` = inverted output). The node changed by an event is tinted with the accent. One layout draws both forms: filled cards and box-drawing lines in text, SVG in graphics terminals.

## Hooks

A HOOK card shows the loop event the hooks ran on (SessionStart, PreCompact, PreToolUse with the call it saw, PostToolUse, Stop, UserPromptSubmit, ...) and one row per script: ✓ ok, ✓ with the lines it added to the context, ⊘ blocked with the reason (a veto, IF-6), ✗ failed with the exit code and stderr tail, ◷ timed out, ◌ running, and how long it took. The frame takes the colour of the worst outcome: warning for a veto or a timeout, error for a failure.
