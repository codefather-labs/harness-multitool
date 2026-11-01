# TUI design concept

Reference mockups for the terminal UI. The implementation matches them pixel for pixel on the terminal cell grid, with colours from `themes/*.toml`. All screens are 1672×941 unless noted. Each screen comes in `-dark` and `-light`.

| Screen | Files | What it fixes |
|---|---|---|
| Splash | `splash-*` | shown first on launch: big logo, name, version, slogan, welcome text, OK (the logo itself comes from `../logo/multitool.svg`, not from the picture) |
| Startup | `startup-*` | after the splash: recent sessions, quick actions, status, tip line; its logo is THE logo |
| Welcome | `welcome-*` | in-session welcome and quick tips |
| Main session | `main-session-*` (1586×992; nord / monokai / cyan 1560×936) | turns, tool rows, diff, status line, composer, `[ Settings ]` |
| Themes | `themes-nord-monokai-cyan.png` (2172×724) | the three extra themes side by side |
| Settings overlay | `settings-overlay-*` | sidebar sections, provider choice incl. local-build plugins |
| Permission | `permission-*` | shell command approval: allow once / always / deny |
| Attachments | `attachments-*` | dropped files as thumbnail chips + preview overlay |
| Inbound messages | `inbound-messages-*` | Telegram, Telegram voice, agent-to-agent, callback, reminder; toast mid-generation |
| Tool calls | `tool-calls-*` | expanded cards: `cat` output with fold, telegram reply, agent_send, journal_add, failure, running |
| Journal & mind maps | `journal-mindmap-events-*` | JOURNAL card, MINDMAP created (empty), inline map after the first nodes |
| Session tabs | `session-tabs-*` | several sessions in one window, `+` for new, a2a hint |
| Onboarding tour | `tour-*` | coach mark over `[ Settings ]`, step 3 of 7 |

Logo: `../logo/multitool.svg`, traced from `startup-light.png` (the operator's pick). Flow on launch: splash -> startup -> session.


`reasoning-*`: a model's reasoning inside its turn, live, folded and unfolded. The build leaves out the mockup's token count: providers report a turn's whole output, not its reasoning apart.

## Designed in code, without a mockup

Settled with the project owner on the live TUI and recorded in the journal; [`../../tui.md`](../../tui.md) lists the rules and the named states that show them.

- **Hook cards** (state `hooks`): the loop event, one row per script with its outcome, the frame in the worst outcome's colour.
- **Map galleries** (state `maps`): every mind-map node kind and every flowchart symbol and logic gate, in text and as SVG.
- **Transcript spacing**: one row between cards with a hairline gap; straight card edges.
- **Startup selection**: an accent outline without a fill, only in the focused panel (the mockup outlines both panels).
- **Tool card text** has no background; only a diff is shaded.

## Known differences to settle before implementation

- `permission-light.png` uses a blue ASSISTANT label and a soft shadow; every other screen uses orange and no shadows. Orange and no shadows win.
- Header logos are hand-drawn approximations; the SVG in `../logo/` is the source of truth.
- Behaviour that a picture cannot show is specified in the project journal: the interactive map viewer (pan, zoom, mouse), the vector rendering rule, session tabs, tool-call cards.
