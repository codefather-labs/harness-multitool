# Roadmap

Each milestone ends with its specifications checked, not with its features written.

| Milestone | Content | Done when |
|---|---|---|
| **M0 — Specification** | This repository's documents; the journal of the project seeded from them; the shared architecture map | every `P-`, `INV-`, `CON-`, `IF-` item is in the journal with its flags; open questions listed |
| **M1 — Walking skeleton** (done) | `api` and `core` crates, registry, event bus, task states, test doubles for every interface; CI builds the core for wasm32 | INV-1, INV-2, INV-5 have passing checks |
| **M2 — MVP** (done on a local model; the Anthropic API not yet called for real) | Anthropic and OpenAI-compatible providers, API-key auth, built-in tools, default policy, file session store, terminal frontend, configuration | a real coding task runs end to end on a native build; INV-3, INV-6 checked |
| **M3 — Memory** (in progress: the store, the agent's tools, the daemon's callback entry, a journal-aware context and compaction are in; a session reaches the store through the daemon when one runs, a Telegram channel is in; remote daemons are not) | the carried-over daemon as `Memory` and `Transport`; journal- and map-aware `ContextStrategy`; callbacks `notification` and `journal` (IF-13) | context built from memory; a long session compacts without losing specifications; INV-12 checked |
| **M4 — Proof** (in: tasks from the terminal UI and the command line, evidence in the journal, acceptance checks in `.harness/acceptance.toml`) | `Verifier`, the done gate, specifications as acceptance checks | INV-4, INV-11 checked |
| **M5 — Browser** | platform-wasm, frontend-web, the demo on the project site | the demo runs a task in the page; INV-10 checked |
| **M6 — Branches and mesh** | rewind and branch, parallel worktree attempts, agent mesh, routing with budgets, `skill` triggers (IF-13, IF-14), callback handlers as plugins with a Callbacks tab in the TUI's settings and hot reload of plugins (IF-16, ADR 0015, CON-50), `flowstate_update` callbacks with live flowchart runs and run history (ADR 0012), step-debug mode (ADR 0013) | ADR 0007 items 3 and 4 shipped with their specifications; INV-13 checked |

**Terminal UI, in progress ahead of the loop:** visible hook runs (ADR 0010), mind-map and flowchart tools with journal tickets and panning (ADR 0011), the welcome screen with the key points; next: SVG export of maps and charts, live flowchart instances (ADR 0012), the Callbacks settings tab (ADR 0015).

**Documentation mirror (ADR 0017, CON-43):** the harness keeps a project's docs tree consistent with its journal, maps and flowcharts.

- The agent writes the files, prompted by a `PostToolUse` hook on every memory write.
- The consistency check is a built-in, id-linked, advisory hook plus `multitool docs check` in CI (CON-45).
- The `project-init` skill creates the tree with an example entry in every file.

**Conversations ([conversations.md](conversations.md)), in progress.** In: sessions in the daemon, the log as the format, transformation between providers, a window's environment, a window taking over a run, and one conversation kept consistent with Claude Code's form in both directions. Left: `multitool run` through the daemon; Codex CLI's form; `multitool chat dump` and `chat load` with `--format`; asking with options and plan mode for a delegate.

**Ideas, not yet on a milestone:**

- generated documentation like rustdoc — `multitool doc`, on demand, builds a linked HTML site and a PDF from the docs files only ([ADR 0018](../decisions/0018-generated-documentation-site.md));
- plan mode — the classic plan mode with the accent on the journal, mind maps and flowcharts, and a hook with their instructions ([ADR 0016](../decisions/0016-plan-mode.md)).

**Concepts, not yet on a milestone:** reliability-monitoring mode — signals accumulate, conditions wake the agent with one message per batch, per-project response policies in `.harness/policies/` (IF-15, INV-14, CON-26; [ADR 0014](../decisions/0014-reliability-monitoring-mode.md)).

## TUI /version (CON-102)

`/version` prints a local VERSION card in the current transcript with `harness-multitool <package version>`. It is listed in `/help` and never sends a model request.

Specification journal#515 replaces journal#504 to repair its documentation identity without changing behaviour. Work journal#505; implementation journal#507; verification journal#509.

Checked:

- frontend-tui: 104 passed, 0 failed, 1 ignored;
- CLI build;
- real PTY preview: `/version` displayed VERSION and harness-multitool 0.1.0-dev, exit 0;
- live dispatch/outbox preservation: checked by unit test. The interactive smoke used preview, not a live provider.

## Hot reload of plugins (M6; ADR 0015, CON-50)

Plugins stay crates linked into the binary; a reload swaps the binary in the same terminal without restarting sessions:

1. **Build.** `cargo build` of the local build. If it fails, nothing is swapped; the agent and the user see the error.

2. **Safe point.** Wait until no model turn is streaming and no tool call is in flight.

3. **Hand-off.** The process writes everything it holds only in memory to a hand-off file: tabs, transcripts, scroll positions, open journal tickets and expanded flowchart instances, the input draft, the open overlay.

4. **Swap in the same terminal.**
   - Linux and macOS: the process gives the terminal back as the shell left it and replaces itself with the new binary (`exec`), which takes the terminal again: the same process and terminal, and the shell does not notice.
   - Windows: a process cannot replace itself, so the old one starts the new binary in the same console, waits for it and passes its exit code on; the console window stays.

5. **Restore.** The new process reads the hand-off, redraws every tab as it was and deletes the file. Callbacks that came during the swap were taken by the daemon and are delivered now.

Done when the test protocol of journal#330 passes on Linux and once on a Windows host.

## Modes with several agents

Beside plan mode, the harness will have two more session modes for work shared between agents. Like plan mode they belong to the harness, whichever adapter runs the agent: the user or the agent switches them, a switch is logged and comes back on a resume, and a mode's rules are in the system prompt only while it is on. Neither is built yet. Both need an agent-to-agent channel and tools for opening tabs, which the harness does not have yet.

- **Brainstorming mode.** The agent gets tools to open several tabs with other agents and bring them into a brainstorm over agent-to-agent messages. They are peers: each says what it thinks of the same question, and answers the others.
- **Orchestration mode.** The agent gets tools to spawn agents in tabs of their own. It is the master and they are its slaves: it gives each its work, follows it, and takes the result.

In both, the master picks the adapter and the model of each agent it starts, from those available in the harness and signed in: a brainstorm can set different models against each other, and an orchestrator can give each piece of work the model that suits it.
