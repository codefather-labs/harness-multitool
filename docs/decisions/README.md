# Decisions

One file per decision: what was decided, by whom, why, and what was rejected. A decision is replaced only by a new one that says so. Status: `accepted`, `concept` (an idea grown into a concept with specifications of its own, not yet scheduled), `proposed`, `replaced by NNNN`.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-plugin-first.md) | Plugin-first: almost every component is an interface | accepted |
| [0002](0002-plugins-are-crates.md) | A plugin is a crate built into the local build, selected in config | accepted |
| [0003](0003-authentication.md) | API-key auth shipped; no subscription login shipped | accepted |
| [0004](0004-platform-layer-and-browser-build.md) | Platform behind interfaces; a WebAssembly build of the CLI on the site | accepted |
| [0005](0005-own-repository.md) | A new, self-contained repository; the memory backend is carried over | accepted |
| [0006](0006-providers.md) | Anthropic API, OpenAI-compatible and local models from v1 | accepted |
| [0007](0007-features-after-mvp.md) | After the MVP: proof, memory-based context, rewind and branches, agent mesh | accepted |
| [0008](0008-name.md) | The name is harness-multitool | accepted |
| [0009](0009-callback-triggers.md) | Callbacks are triggers: `message`, `skill`, `journal` | accepted |
| [0010](0010-hook-runs-are-visible.md) | Hook runs are events, and frontends show every one | accepted |
| [0011](0011-mind-maps-and-flowcharts.md) | Mind maps and flowcharts are separate tools | accepted |
| [0012](0012-flowcharts-as-live-pipelines.md) | Flowcharts are live: `flow` callbacks, runs, run history | accepted, open to additions |
| [0013](0013-step-debug-mode.md) | Step-debug mode: charts, callbacks and a debug router the agent builds in the target project | accepted, open to additions |
| [0014](0014-reliability-monitoring-mode.md) | Reliability-monitoring mode: metrics, SLOs and a response policy per event | concept |
