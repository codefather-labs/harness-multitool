# 0012 — Flowcharts are live: flow callbacks, runs, run history

- **Status:** accepted, open to additions — the owner described the goal and may extend it; the mechanism is partly open (Q-12, Q-13; Q-11 settled below) · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-7, IF-11, IF-13, CON-23, INV-12, INV-13; builds on [ADR 0009](0009-callback-triggers.md) and [ADR 0011](0011-mind-maps-and-flowcharts.md)

## Decision

A flowchart is not only documentation: it can be bound to a real process or pipeline of the project the agent works on (a CI job, a deploy, a data pipeline, a release procedure, a service's request path) and show it **live**. A fourth kind of callback trigger, **`flow`**, exists for this.

**Binding.** A flowchart is bound to the process it describes, and to the windows that watch it: a specific session (tab) of a specific agent. The binding is configuration, like other triggers (CON-23).

**Flow events.** A `flow` callback names the bound flowchart, a **run** (one pass of the process through the chart: a deploy, a CI build, one request), and a step: a node or an arrow of the chart, with a state — `entered`, `passed`, `failed`, `skipped`, `waiting` — and optional data (a duration, a short message, a link to a log).

**Live state.** Every watching window shows the chart's current run: the active node highlighted, the path already taken marked, a failed node in the error colour, the time spent per node. Every `flowstate_update` event that changes the chart **renders it in the terminal**: the chart is drawn into the transcript the same way the `/flowchart` command draws it, already in its new state — active node, path taken, failures. The user sees the pipeline move step by step: the work is visibly going on. (Decided by the owner, 2026-11-01; replaces the earlier "updated in place, not appended on every event" rule.)

**Runs are instances of a reference chart — decided by the owner (answers Q-11).** The flowchart the agent draws for a pipeline is its **reference**. One new pass of the pipeline is one new **instance**: a new flowchart made from the reference, with its own state. The client identifies the pass by its own unique id — a `correlation_id` of a request chain or any other identifier its product logic guarantees — and sends it with every `flowstate_update` of that pass; that id ties events to their instance, idempotently from the client to the instance. **Several instances render collapsed — decided by the owner, replacing "the latest is expanded".** When a chart has several instances, every instance is drawn **collapsed by default**, to one line; the user opens any of them, and any number of them at once. When an update comes into a collapsed instance, its line is highlighted with a **soft one-second pulse** — the accent colour fades in and out, not the terminal's blink attribute — so the user sees that it moved. Confirmed by the owner: with several instances each one is one card updated in place (no card per event); a single instance — the usual case in step-debug mode — keeps the render-per-event behaviour above, a new card per event.

**Run history.** Every event of every run is kept, in order, with its time (INV-13). Runs can be listed, opened and replayed step by step; two runs can be compared. This makes a chart a tool for **retrospective validation**: did the callbacks fire as designed, did the process take the path the chart says, where does it stall or fail, which runs differed.

**Checks the chart enables.** An event naming a node or an arrow that is not in the chart, or a step that does not follow an arrow from the run's current node, is recorded as an **off-chart** event and shown as such — the process and its chart disagree. A run that reaches an end node is complete; a run without events for longer than its node allows is stalled.

So the agent can document a pipeline as a flowchart, instrument the pipeline to send flow callbacks at its steps, and then watch and debug it as it runs, and afterwards from its history: event-driven, live debugging of any process of the project.

## Boundaries

- A `flow` callback changes only the run's state and history. It runs no tool and starts no agent turn; a process that must also wake the agent uses a `message` or `skill` trigger as well (IF-13).
- INV-12 holds: the event's payload is data. It cannot change the chart, the binding or the trigger configuration; an unknown step is recorded as off-chart, never added to the chart.
- Callers are authenticated per caller like every trigger (IF-13).

## Why

The owner wants charts to serve beyond documentation: as the live view and the flight recorder of the processes the agent builds and runs. A chart already holds the intended flow; checking real events against it turns every deviation into something visible and every past run into evidence.

## Open

How arbitrary payloads map to node ids, retention and size of the run history, and how a watching window is chosen (Q-12, Q-13). The event schema is the callback contract (`uuid`, `type`, the client's pipeline id; IF-13). The owner left the mechanism open on purpose.

Used by [ADR 0013](0013-step-debug-mode.md): step-debug mode builds on live charts — the active node is the one the latest flow callback came from, and a run that reaches a terminal block is saved to the project's `.harness/debug/` SQLite and the chart reset.

**Clarified by the owner, 2026-11-01:** a chart changes visually only when callbacks come for it; building those callback calls into the client's code is the agent's task. For now a chart's state driven by events serves only to visualise how a project's pipeline works. Mechanisms that act on a chart's nodes — automations fired at a node — are an idea for later, not part of this decision.
