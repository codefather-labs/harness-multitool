# Open questions

| Id | Question | Needs |
|---|---|---|
| ~~**Q-1**~~ | ~~The name.~~ Settled: **harness-multitool** ([ADR 0008](../decisions/0008-name.md)). | — |
| **Q-2** | The license (MIT, Apache-2.0, or both). | the owner |
| **Q-3** | Configuration format and location (TOML in the project and in the user's config directory?). | design |
| **Q-4** | Do the chosen providers accept direct requests from a browser (CORS), and with which headers? | a check per provider |
| **Q-5** | Which ratatui backend renders to the DOM well enough for the demo? | a spike |
| **Q-6** | Out-of-process plugins in any language: when, and for which interfaces? | after M2 |
| **Q-7** | A sandbox for third-party plugins (WebAssembly components)? | after M4 |
| **Q-8** | Is "no harness runs in the browser" true? Check before saying it publicly. | research |
| **Q-9** | Trigger configuration: the match syntax (label globs, payload field conditions), how a payload maps onto a journal entry or a skill's input (templates?), and per-trigger rate limits. | design |
| **Q-10** | How a `skill` run started by a callback reports back to the caller: the callback's result URL, a reply callback, or a message to the user's channel? | design |
| ~~**Q-11**~~ | ~~How a run is identified across flow events.~~ Settled by the owner: a pass is an instance of the reference chart, identified by the client's own unique pipeline id (a `correlation_id` or any id its product logic guarantees) sent with every `flowstate_update`; several instances render collapsed by default, the user opens any, a collapsed instance pulses for one second on an update ([ADR 0012](../decisions/0012-flowcharts-as-live-pipelines.md)). | — |
| **Q-12** | How events of an existing process map onto a chart's nodes without changing the process: a mapping table in the binding, labels, payload fields? | design |
| **Q-13** | Run history: retention, size limits, and which windows watch a chart (a named session, every session of the project, the one that created it). | design |
| ~~**Q-14**~~ | ~~How the debug router receives the agent's decisions.~~ Settled by the owner: the debug router is a router in the client app, built by one OpenAPI-style contract — `approve`, `step back` only where possible, product routes; env-gated and authorized (CON-25, CON-27). The harness's own router is its daemon, the single callback entry point. | — |
| ~~**Q-16**~~ | ~~What a waiting step does when no decision comes.~~ Settled by the owner: it waits for `approve`, no timeouts, nothing on its own even if the agent is gone; a health check was rejected as over-engineering (CON-29). | — |
| ~~**Q-17**~~ | ~~Where reliability metrics come from and how they reach the agent.~~ Settled by the owner: from anywhere, by any transport, accumulated; the agent gets a message only when a `Condition` (IF-15, any logic, always a `bool`) turns `true` ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-18**~~ | ~~The interface for writing a project's response policies.~~ Settled by the owner: the client describes the policies, the agent writes them as Markdown files in the project, and each condition links the paths of its policy files ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-19**~~ | ~~What an agent may do on its own and how humans are paged.~~ Settled by the owner: described per project in its policies; not unified at the harness level ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-20**~~ | ~~A days-long watch versus short sessions.~~ Settled by the owner: monitoring sessions stay open 24/7 ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-21**~~ | ~~Does a condition that stays `true` resend?~~ Settled by the owner: a condition is idempotent — one message per event; repeats go to the harness's trigger log, and the message tells the agent to check that log for nearby triggers (INV-14). | — |
| ~~**Q-22**~~ | ~~What identifies "the same event".~~ Settled by the owner: the incoming event uuid, a required part of the communication contract; a record whose uuid is already recorded is the same event (INV-14). | — |
| ~~**Q-23**~~ | ~~A burst of different events on one condition.~~ Settled by the owner: one message per batch; firing rules live in the callback plugin, written by the user so as not to spam the agent (INV-14). | — |
| **Q-24** | Hot reload of plugins and callback handlers without restarting sessions (ADR 0015) against ADR 0002 (a plugin is a crate linked into the binary): the mechanism — dynamic libraries (Rust has no stable ABI), WebAssembly components (Q-7), out-of-process plugins (Q-6), or re-executing the binary while sessions survive in the daemon. ADR 0002 must be amended to fit. | design, the owner |
