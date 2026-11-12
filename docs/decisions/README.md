# Decisions

One file per decision: what was decided, by whom, why, and what was rejected. A decision is replaced only by a new one that says so. Status: `accepted`, `concept` (an idea grown into a concept with specifications of its own, not yet scheduled), `proposed`, `replaced by NNNN`.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-plugin-first.md) | Plugin-first: almost every component is an interface | accepted |
| [0002](0002-plugins-are-crates.md) | A plugin is a crate built into the local build, selected in config | accepted, amended by 0015 |
| [0003](0003-authentication.md) | API-key auth shipped; no subscription login shipped | accepted |
| [0004](0004-platform-layer-and-browser-build.md) | Platform behind interfaces; a WebAssembly build of the CLI on the site | accepted |
| [0005](0005-own-repository.md) | A new, self-contained repository; the memory backend is carried over | accepted |
| [0006](0006-providers.md) | Anthropic API, OpenAI-compatible and local models from v1 | accepted |
| [0007](0007-features-after-mvp.md) | After the MVP: proof, memory-based context, rewind and branches, agent mesh | accepted |
| [0008](0008-name.md) | The name is harness-multitool | accepted |
| [0009](0009-callback-triggers.md) | Callbacks are triggers: `notification`, `skill`, `journal` | accepted, amended by 0015 |
| [0010](0010-hook-runs-are-visible.md) | Hook runs are events, and frontends show every one | accepted |
| [0011](0011-mind-maps-and-flowcharts.md) | Mind maps and flowcharts are separate tools | accepted |
| [0012](0012-flowcharts-as-live-pipelines.md) | Flowcharts are live: `flowstate_update` callbacks, runs, run history | accepted, open to additions |
| [0013](0013-step-debug-mode.md) | Step-debug mode: charts, callbacks and a debug router the agent builds in the target project | accepted, open to additions |
| [0014](0014-reliability-monitoring-mode.md) | Reliability-monitoring mode: metrics, SLOs and a response policy per event | concept |
| [0015](0015-callback-handlers-are-plugins.md) | Callback handlers are plugins: one interface for the standard and custom types, plugged in through the TUI's settings | accepted |
| [0021](0021-agent-behaviour.md) | How the agent behaves: plan first, then the mode the work needs | accepted |
| [0022](0022-claude-code-conversations.md) | Moving a Claude Code conversation onto the harness | accepted |
| [0023](0023-the-log-is-the-conversation.md) | The log is the conversation; each provider's form is written from it | accepted |
| [0024](0024-conversations-live-in-the-daemon.md) | Conversations live in the daemon | accepted |
| [0025](0025-one-conversation-in-every-form.md) | One conversation in every form | accepted |
| [0026](0026-profiles.md) | Profiles: every setting, and each window's sign-in | proposed |
| [0027](0027-windows-talk-to-the-daemon-over-a-local-socket.md) | Windows talk to the daemon over a local socket | accepted |
| [0020](0020-claude-code-delegate.md) | Claude Code as a delegate agent, outside the default build | proposed |
| [0019](0019-vendor-documented-sign-in.md) | A subscription sign-in ships where the vendor documents it for other apps | accepted |
| [0016](0016-plan-mode.md) | Plan mode: the classic plan mode with the accent on the journal, mind maps and flowcharts | accepted |
| [0017](0017-docs-mirror-memory.md) | Project initialization (docs tree + `.harness/`); documentation mirrors the journal, mind maps and flowcharts | accepted |
| [0018](0018-generated-documentation-site.md) | Generated documentation like rustdoc: a linked HTML site and a PDF from the docs, maps and charts | proposed (an idea) |
| [0028](0028-journal-todos-as-tracker-issues.md) | Journal todos become issues in a tracker, GitHub first, per project | accepted (journal#971); built: journal#990, journal#993 |
| [0029](0029-agent-games.md) | Games between agents on the daemon, Texas hold'em first, with a dashboard | proposed |
| [0030](0030-harness-skills.md) | Skills of the harness's own (.harness/skills), the same on every adapter | accepted (the owner's standing approval, journal#971) |
