# 0013 — Step-debug mode

- **Status:** accepted, open to additions · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-4, IF-6, IF-7, IF-11, IF-13, CON-24, CON-25, INV-13; builds on [ADR 0009](0009-callback-triggers.md) (callback triggers) and [ADR 0012](0012-flowcharts-as-live-pipelines.md) (live flowcharts)

## Decision

The harness has a **step-debug mode**: event-driven, step-by-step debugging of the project the agent works on, like the stepping debugger of an IDE, but run by the agent through its harness. The owner has used this way of debugging with agents for a long time; the harness makes it a mode of its own.

**Not for every project or task.** The mode pays off for large, complex pipelines, above all in distributed systems: many services, queues, agents, where a step's effect shows up far from its cause and no ordinary debugger spans the whole path. For a small program, a single process or a local bug, ordinary debugging, tests and logs are cheaper: instrumenting code, building charts and a debug router costs more than it gives. The mode is opt-in and off by default.

**Switching.** The agent switches a session into step-debug mode when the operator asks, or on its own when the task is debugging such a pipeline; it does not switch on its own for other tasks. Leaving the mode is the same. The switch is an event, visible in the session (IF-11).

**A hook sets the agent's rules.** Switching the mode fires a hook (IF-6) that adds the step-debug instructions to the agent's context. While the mode is on, the agent follows them:

1. **Charts.** The agent builds a flowchart for each pipeline it debugs, from the pipeline's design. Example: the project fbscout has two large pipelines — one of services joined by Kafka (the software side), one of agent events that take an input, do something and return a result — so two flowcharts.
2. **Callbacks.** The agent instruments the project's code with callbacks into its own harness, at every place it needs, of three kinds:
   - **information** — brings data to the agent (a `message` trigger, ADR 0009);
   - **journal** — only records the event in the journal (a `journal` trigger);
   - **flow** — changes the state of the pipeline's flowchart (a `flow` trigger, ADR 0012): it updates the run's data and marks the node it came from as active. The active node is always the one the most recent callback came from.
3. **Stepping lives in the target project.** The harness does not pause anyone's code. The agent makes the pipeline stepwise itself: after each callback the pipeline **waits for the agent's decision** before it goes on. For this the agent builds a **debug router** in the target project that lets a pipeline, step by step as its flowchart says, execute its next step only when the agent allows it. The harness gives the agent a tool to send that decision (continue, stop) to the router.

**End of a cycle.** When a run reaches a terminal block of the flowchart, the cycle ends: the harness saves the run — its state and its history (INV-13) — into an SQLite database of the project under `.harness/debug/`, then resets the chart for the next cycle.

So the mode turns a flowchart into a live map of a running pipeline that the agent walks through step by step: it sees where the pipeline is, what each step reported, decides whether it goes on, and keeps every cycle for later.

**Strictly behind an environment gate.** Everything the mode puts into the target project — the debug router, the step waits, the mode's callbacks — is active only when the project runs with `MULTITOOL_STEP_DEBUG=1`. Without it the code is inert: no callback is sent, no step waits, the pipeline runs as it would without the instrumentation. The gate keeps the debug code from stalling a production pipeline it was shipped into by mistake (proposed by the agent, decided by the owner, 2026-11-01; CON-25).

## Why

Logs tell what happened after the fact; a debugger stops the program where it runs. Pipelines spread over services, queues and agents have no debugger that spans them. Callbacks into the harness plus a router that waits for the agent give that: the agent sees each step on the chart as it happens and decides the next one, and every cycle is kept to check afterwards.

## Open

How the debug router receives the agent's decisions (Q-14), what a waiting step does when no decision comes (Q-16). The owner may add to this decision.
