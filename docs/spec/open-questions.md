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
| **Q-11** | Flow callbacks: how a run is identified across events (an id the caller sends, or one the harness hands out on the first event), and the event schema (node or arrow, state, data). | design |
| **Q-12** | How events of an existing process map onto a chart's nodes without changing the process: a mapping table in the binding, labels, payload fields? | design |
| **Q-13** | Run history: retention, size limits, and which windows watch a chart (a named session, every session of the project, the one that created it). | design |
| **Q-14** | Step-debug: how a target project's debug router receives the agent's decisions (it polls the harness, a callback reply, a socket, a file) and what a decision can carry (continue, stop, skip, changed input?). | design |
| **Q-16** | Step-debug: what a waiting step does when no decision comes (wait forever, time out and continue, time out and stop), and how several pipelines step at once. | design |
| ~~**Q-17**~~ | ~~Where reliability metrics come from and how they reach the agent.~~ Settled by the owner: from anywhere, by any transport, accumulated; the agent gets a message only when a `Condition` (IF-15, any logic, always a `bool`) turns `true` ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| **Q-18** | Reliability monitoring: the interface for writing a project's response policies. Policies are built per project ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)); the authoring interface is not designed yet. | the owner, design |
| ~~**Q-19**~~ | ~~What an agent may do on its own and how humans are paged.~~ Settled by the owner: described per project in its policies; not unified at the harness level ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-20**~~ | ~~A days-long watch versus short sessions.~~ Settled by the owner: monitoring sessions stay open 24/7 ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). | — |
| ~~**Q-21**~~ | ~~Does a condition that stays `true` resend?~~ Settled by the owner: a condition is idempotent — one message per event; repeats go to the harness's trigger log, and the message tells the agent to check that log for nearby triggers (INV-14). | — |
| **Q-22** | Conditions: what identifies "the same event" and when a condition re-arms. Proposed by the agent: by default one event lasts while the condition stays `true`; a condition may name its event more precisely (e.g. a pipeline iteration's id). | the owner, design |
