# TUI design concept

Reference mockups for the terminal UI. The implementation matches them pixel for pixel on the terminal cell grid, with colours from `themes/*.toml`. All screens are 1672×941 unless noted. Each screen comes in `-dark` and `-light`.

| Screen | Files | What it fixes |
|---|---|---|
| Startup | `startup-*` | first screen after launch: logo, recent sessions, quick actions, tip line |
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

Logo: `../logo/multitool.svg` (colour), `multitool-mono.svg`, `multitool-16.svg`.

`drafts/` keeps rejected or superseded variants (old logo, layouts that drifted from the system). They are history, not reference.

## Known differences to settle before implementation

- `permission-light.png` uses a blue ASSISTANT label and a soft shadow; every other screen uses orange and no shadows. Orange and no shadows win.
- Header logos are hand-drawn approximations; the SVG in `../logo/` is the source of truth.
- Behaviour that a picture cannot show is specified in the project journal: the interactive map viewer (pan, zoom, mouse), the vector rendering rule, session tabs, tool-call cards.
