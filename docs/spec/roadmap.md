# Roadmap

Each milestone ends with its specifications checked, not with its features written.

| Milestone | Content | Done when |
|---|---|---|
| **M0 — Specification** | This repository's documents; the journal of the project seeded from them; the shared architecture map | every `P-`, `INV-`, `CON-`, `IF-` item is in the journal with its flags; open questions listed |
| **M1 — Walking skeleton** | `api` and `core` crates, registry, event bus, task states, test doubles for every interface; CI builds the core for wasm32 | INV-1, INV-2, INV-5 have passing checks |
| **M2 — MVP** | Anthropic and OpenAI-compatible providers, API-key auth, built-in tools, default policy, file session store, terminal frontend, configuration | a real coding task runs end to end on a native build; INV-3, INV-6 checked |
| **M3 — Memory** | the carried-over daemon as `Memory` and `Transport`; journal- and map-aware `ContextStrategy`; callback triggers `message` and `journal` (IF-13) | context built from memory; a long session compacts without losing specifications; INV-12 checked |
| **M4 — Proof** | `Verifier`, the done gate, specifications as acceptance checks | INV-4, INV-11 checked |
| **M5 — Browser** | platform-wasm, frontend-web, the demo on the project site | the demo runs a task in the page; INV-10 checked |
| **M6 — Branches and mesh** | rewind and branch, parallel worktree attempts, agent mesh, routing with budgets, `skill` triggers (IF-13, IF-14), callback handlers as plugins with a Callbacks tab in the TUI's settings and hot reload of plugins (IF-16, ADR 0015, CON-50), `flow` triggers with live flowchart runs and run history (ADR 0012), step-debug mode (ADR 0013) | ADR 0007 items 3 and 4 shipped with their specifications; INV-13 checked |

**Terminal UI, in progress ahead of the loop:** visible hook runs (ADR 0010), mind-map and flowchart tools with journal tickets and panning (ADR 0011), the welcome screen with the key points; next: SVG export of maps and charts, live flowchart instances (ADR 0012), the Callbacks settings tab (ADR 0015).

**Documentation mirror (ADR 0017, CON-43):** the harness keeps a project's docs tree consistent with its journal, maps and flowcharts — how (agent-written, generated, checked) is Q-27.

**Ideas, not yet on a milestone:** plan mode — the classic plan mode with the accent on the journal, mind maps and flowcharts, and a hook with their instructions ([ADR 0016](../decisions/0016-plan-mode.md)).

**Concepts, not yet on a milestone:** reliability-monitoring mode — signals accumulate, conditions wake the agent with one message per batch, per-project response policies in `.harness/policies/` (IF-15, INV-14, CON-26; [ADR 0014](../decisions/0014-reliability-monitoring-mode.md)).

## Hot reload of plugins (M6; ADR 0015, CON-50)

Plugins stay crates linked into the binary; a reload swaps the binary in the same terminal without restarting sessions:

1. **Build.** `cargo build` of the local build. If it fails, nothing is swapped; the agent and the user see the error.
2. **Safe point.** Wait until no model turn is streaming and no tool call is in flight.
3. **Hand-off.** The process writes everything it holds only in memory to a hand-off file: tabs, transcripts, scroll positions, open journal tickets and expanded flowchart instances, the input draft, the open overlay.
4. **Swap in the same terminal.**
   - Linux and macOS: the process replaces itself with the new binary (`exec`) — the same process and terminal, raw mode and the alternate screen kept; neither the shell nor the terminal notices.
   - Windows: a process cannot replace itself, so the old one starts the new binary in the same console, waits for it and passes its exit code on; the console window stays.
5. **Restore.** The new process reads the hand-off, redraws every tab as it was and deletes the file. Callbacks that came during the swap were taken by the daemon and are delivered now.

Done when the test protocol of journal#330 passes on Linux and once on a Windows host.
