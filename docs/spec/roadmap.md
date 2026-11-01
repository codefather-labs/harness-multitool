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
| **M6 — Branches and mesh** | rewind and branch, parallel worktree attempts, agent mesh, routing with budgets, `skill` triggers (IF-13, IF-14), `flow` triggers with live flowchart runs and run history (ADR 0012), step-debug mode (ADR 0013) | ADR 0007 items 3 and 4 shipped with their specifications; INV-13 checked |
