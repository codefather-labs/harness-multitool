# Constraints

Rules, limits and constants. Unlike a principle, a constraint is not weighed against anything; unlike an invariant, it is a choice that could have been different and is recorded as one.

## Technology

| Id | Constraint |
|---|---|
| **CON-1** | The harness is written in Rust, stable toolchain. |
| **CON-2** | A plugin is a crate that implements an interface trait. It is compiled into the binary of a local build and selected by name in the configuration. |
| **CON-3** | No dynamically loaded Rust libraries as plugins (Rust has no stable ABI). |
| **CON-4** | Out-of-process plugins (any language, over stdio) are not part of v1. They may be added later as another way to implement the same interfaces. |
| **CON-5** | Build targets: Linux x86_64 and arm64, macOS arm64, Windows x86_64, and `wasm32-unknown-unknown` for the browser demo. |
| **CON-6** | The terminal interface is built on ratatui. |

## Providers and authentication

| Id | Constraint |
|---|---|
| **CON-10** | v1 providers: the Anthropic API, OpenAI-compatible APIs (OpenAI, OpenRouter and the like), and local models served over an OpenAI-compatible endpoint. |
| **CON-11** | Shipped `Auth` implementations: API keys (from the environment, the config, or the OS keychain on native targets) and `gpt-subscription` (CON-55). See INV-9 for what is not shipped. |

## Memory and sessions

| Id | Constraint |
|---|---|
| **CON-20** | The default `Memory` implementation is a local daemon that owns a project journal (entries with categories, tags, flags and links, hybrid lexical + semantic search), mind maps and flowcharts. The same daemon is the single entry point of callbacks to agents, with a callback URL for every open agent window (IF-13). In the browser, the same interface is implemented over browser storage. |
| **CON-21** | Journal flags include work items (todo, question, problem, bugreport, reminder, unverified, disputed) and marks (specification, constraint, important, idea, concept, comment, verified, deprecated); an entry carries any number of them. |
| **CON-22** | One journal per project; a project is identified by its repository remote, or its directory when it has none. |
| **CON-23** | Callback triggers (IF-13) are configuration of the project, versioned with it; every callback names its `type` (IF-13), and the harness types are `notification` (a callback delivered into the agent's input, its stdin — the inbound CALLBACK card the TUI shows; `/callback` publishes one), `journal` (only writes a journal entry from the event; no agent turn), `flowstate_update` (changes the state of one instance of a flowchart — a pass keyed by the client's pipeline id, never the reference chart; no agent turn) and `skill` (starts a run of a named skill with the event as input; kept as a concept, may be dropped); product types are any string, with custom handlers (IF-16); a flowchart's binding to a process and to its watching sessions is configuration too. A callback whose type has no handler reaches no agent: it is logged and the caller gets 400 (ADR 0015). Neither a `journal` nor a `flowstate_update` callback starts a model turn. |
| **CON-24** | Step-debug runs are stored in the target project, in an SQLite database under `.harness/debug/`: one run per cycle, saved when the run reaches a terminal block of its flowchart. |
| **CON-25** | Step-debug code in a target project (debug router, step waits, the mode's callbacks) is active strictly behind the environment gate `MULTITOOL_DEBUG_MODE=1` and only with authorization — checked by the app itself or by a token issued by the harness daemon; without the gate the code is inert and the pipeline runs as uninstrumented ([ADR 0013](../decisions/0013-step-debug-mode.md)). |
| **CON-26** | A project's response policies (reliability monitoring) are Markdown files under `.harness/policies/` in the project; conditions link their paths ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). |
| **CON-27** | The step-debug router of a target app is always built by one contract, an OpenAPI-style specification the harness ships: an `approve` route that lets the app go to its next step; a `step back` route only where stepping back is really possible — otherwise not implemented at all, no stubs; the app's other product routes as the product needs ([ADR 0013](../decisions/0013-step-debug-mode.md)). |
| **CON-28** | A debug-router route for debugging product behaviour or invariants imports and calls the functions the real code calls; it implements no business logic of its own. A route that reimplements logic instead of calling the code it debugs breaks the contract ([ADR 0013](../decisions/0013-step-debug-mode.md)). |
| **CON-29** | A step-debug pipeline waiting after a callback advances only on the agent's `approve`: no timeout, no default action, nothing on its own if the agent or its session is gone ([ADR 0013](../decisions/0013-step-debug-mode.md)). |

## Browser build

| Id | Constraint |
|---|---|
| **CON-30** | The browser build has no shell, no processes and no daemon: files live in an in-memory or origin-private file system, network goes through `fetch`, storage through IndexedDB. |
| **CON-31** | The user's key in the browser build is entered by the user and stored only in that browser. |

## Project

| Id | Constraint |
|---|---|
| **CON-40** | The repository is self-contained: it does not refer to, depend on or name the project it grew out of. |
| **CON-41** | Every spec item, invariant and constraint has a stable id; ids are never reused. |
| **CON-42** | A change to a principle, invariant, constraint or interface contract goes through a decision record in `docs/decisions/`. |
| **CON-43** | Once a project has its docs tree (created by the `project-init` skill), its journal, mind maps and flowcharts are mirrored into Markdown files in its repository — README, architecture, spec (principles, interfaces, invariants, constraints, open questions, roadmap), decisions, flowchart pages — and kept consistent with them in the same change; the agent writes the files, prompted by a `PostToolUse` hook on every memory write; a disagreement is a finding settled before work goes on ([ADR 0017](../decisions/0017-docs-mirror-memory.md)). |
| **CON-44** | Project initialization (`project-init`) creates the docs tree and the `.harness/` folder; it never overwrites an existing file. Everything in `.harness/` is committed with the project, for now. The harness asks about it once, at the first start in a project without `docs/` and `.harness/` ([ADR 0017](../decisions/0017-docs-mirror-memory.md)). |
| **CON-45** | The docs-mirror check is advisory and structural: a built-in hook plugin on typed memory-write events, linked by doc id (the file named by the category contains the id; settled and deprecated items are marked); a `Stop` hook adds one reminder per turn and never vetoes, forces continuation or loops; the same checker runs as `multitool docs check` and in CI. It does not judge whether the prose agrees ([ADR 0017](../decisions/0017-docs-mirror-memory.md)). |
| **CON-46** | The documentation files are a storefront: only the current, settled state, written to be read. No drafts, work notes, scratch text, TODO placeholders, half-written sections, superseded variants kept for history, or assumptions written as facts; work in progress lives in the journal, maps and charts, history in git. What is not settled appears only in its declared form: an open question, an ADR with status `proposed` or `concept`, a labelled proposal ([ADR 0017](../decisions/0017-docs-mirror-memory.md)). |

## Plugins and callbacks

| Id | Constraint |
|---|---|
| **CON-50** | A hot reload swaps the binary only at a safe point — no model turn streaming, no tool call in flight — and never loses the view: everything held only in memory goes to a hand-off file the new process restores; a failed build swaps nothing ([ADR 0015](../decisions/0015-callback-handlers-are-plugins.md)). |
| **CON-51** | A project's `.harness/acceptance.toml` runs only in the exact text the user approved; any change, by anyone, asks again. Commands the user gives for a task need no approval. Commands in journal entries never run. |
| **CON-52** | The Telegram channel delivers only messages from `allowed_users`, as data marked external (INV-12) under a label a callback cannot claim. Its token is the harness's own, and appears in no log, status or error. No token means nothing polls. A reply goes to the chat of the last allowed message and asks permission first. |
| **CON-53** | In plan mode the engine refuses writes and every command but a single known read-only one, before hooks and policy, so no answer can approve them ([ADR 0016](../decisions/0016-plan-mode.md)). Only the user ends the mode. |
| **CON-54** | A permission prompt takes only Deny for its first 800 ms on screen: it can open at any moment, started by a callback or a channel, and keys meant for the composer must not approve it. |
| **CON-55** | A ChatGPT plan is reached only by OpenAI's documented flow for open-source apps: the harness registers a client of its own, the user signs in on OpenAI's page, the ID token and the plan scope are validated before anything is saved, tokens are owner-only and in no log, and requests go to `POST /v1/responses` with `store` false and `stream` true. |
| **CON-56** | Claude subscription credentials are not supported: Anthropic does not permit third-party apps to offer Claude.ai login or to collect, store or route its tokens ([its terms](https://code.claude.com/docs/en/legal-and-compliance)). The `claude-subscription` adapter exists only to refuse: it holds no token logic, the settings list it and answer with a toast. Anthropic models take a Console API key or a cloud provider. |
