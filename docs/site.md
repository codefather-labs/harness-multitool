# The site: harness-multitool.dev

**Status: planned (CON-184).** This page describes what is being built.

The project's site is at https://harness-multitool.dev. It has three parts, top to bottom.

1. **The first screen.** The logo, drawn as the brand mark ([logo](design/logo/multitool.svg)), the name and the slogan. Under them, the install line, in the style of rustup:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://harness-multitool.dev/install.sh | sh
   ```

   It has a copy button, and a line under it for Windows (`install.ps1`) and for building from source.

2. **The multitool in the browser.** It sits directly under the install line. It is the real terminal interface (`crates/frontend-tui`) compiled to `wasm32-unknown-unknown` and drawn by a ratatui web backend (Ratzilla), with no network.
   - **Demo**: a scripted session plays by itself and loops, with a progress bar. It shows a message and its answer, a tool call, the journal, a mind map, Settings > Callbacks, a theme switch and the plan approval card.
   - **Explore yourself**: your keys and clicks go to the interface. A mocked agent answers what you send.
   - A toggle above the frame switches between the two. A key or a click in the frame during the demo switches to Explore.

3. **The feature cards, the documentation and the footer.** The documentation, under `/docs/`, is generated from this repository's `docs/`: the [terminal UI](tui.md), [tools and modes](tools.md), [callbacks](callbacks.md), the [agent workflow](agent-workflow.md), the specifications and the decisions. The site never holds a second copy of a doc.

## Design

The mockup came from the design chat that drew the TUI's screens. It is kept in [design/site](design/site/): `landing-dark-top.jpg` and `landing-dark-bottom.jpg`.

Two departures from the mockup:
- the frame shows the real interface, not the mockup's drawing of it;
- the command is `multitool`, not `harness-multitool`.

## How it is built

| Part | Where |
|---|---|
| The interface for the browser | `crates/frontend-web`: frontend-tui on Ratzilla, the mock agent and the demo script |
| The pages | `site/`: the landing template, CSS and JS. `crates/site` renders `docs/*.md` into `/docs/` |
| The install line | a POSIX `sh` bootstrap at `/install.sh` |
| Hosting, binaries and DNS | the owner's choice, journal#1390 |
| Deploy | CI, from `main` |
