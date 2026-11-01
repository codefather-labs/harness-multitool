# Interfaces

Every pluggable component, with what it must do (behaviour), what it may touch (boundary), and its default implementation. An implementation is selected by name in the configuration:

```toml
[plugins]
provider = "anthropic"
auth     = "api-key"
memory   = "daemon"
tools    = ["read", "edit", "shell", "search", "my-deploy-tool"]
```

The names are illustrative; the configuration format itself is an open question.

## Core (not pluggable)

| Id | Spec |
|---|---|
| **IF-0.1** | **Agent loop.** One task runs as turns: build context → model turn (streamed) → tool calls, each through `Policy` → tool results → next turn, until the model stops and `Verifier` passes, a budget runs out, or the user stops it. |
| **IF-0.2** | **Event bus.** Every step of the loop is an event (turn started, token streamed, tool requested, tool approved or denied, tool result, verification result, task state change). Plugins observe events; `Hook` plugins may also veto or amend before-events. |
| **IF-0.3** | **Registry.** Resolves `(interface, name)` from the configuration to an implementation linked into the build; an unknown name is a startup error naming the interfaces' known implementations. |
| **IF-0.4** | **Task states:** `planned → running → verifying → done` or `failed` / `abandoned`; `done` only through `verifying` (INV-4). |
| **IF-0.5** | **Session modes.** A session runs in a mode: normal, or **step-debug** ([ADR 0013](../decisions/0013-step-debug-mode.md)), off by default and meant for large, complex pipelines, above all distributed ones — not for every project or task. The agent switches the mode when the operator asks, or on its own only when the task is debugging such a pipeline; the switch is an event and fires a hook that adds the mode's instructions to the agent's context. Step-debug code in the target project is gated by `MULTITOOL_STEP_DEBUG=1` (CON-25). Proposed, an idea only: a **reliability-monitoring** mode — metrics come in, and each threshold or SLO event has a response policy ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). |

## Model

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-1** | `Provider` | Streams a model turn: text, tool calls, usage (input/output/cache tokens) and cost; lists models and their limits (context window, tool support). | Network to its own endpoint only; credentials only through `Auth`. | Anthropic, OpenAI-compatible, local (CON-10) |
| **IF-2** | `Auth` | Produces credentials for a provider request; refreshes them when they expire. | The only component that sees secrets (INV-6). | API key (CON-11) |
| **IF-3** | `Router` | Picks the provider and model for each step (e.g. a cheap model to read and search, a strong one to decide and write) and enforces budgets in tokens, cost and time per task. | Reads usage events; cannot call tools. | One model for everything |

## Action

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-4** | `Tool` | Declares a name, a description, a JSON schema for its arguments and the capabilities it needs; executes a call and returns a result (text, structured data, files). Users write their own. | Only the capabilities it declared and `Policy` approved (INV-3); platform access through Platform interfaces (INV-1). | read, edit, shell, search, web fetch; journal, mind map and flowchart tools over `Memory`; in step-debug mode, a tool that sends the agent's decision (continue, stop) to a target project's debug router |
| **IF-5** | `Policy` | Decides whether a tool call may run: allow, deny, or ask the user; per tool, per path, per command, per host; remembers answers within a scope. | Sees calls and declarations, not secrets. | Ask for writes, shell and network; allow reads inside the project |
| **IF-6** | `Hook` | Runs on loop events; may veto or amend a before-event (e.g. block a commit without a test run). A session mode switch fires a hook too: entering step-debug mode (IF-0.5) adds the step-debug instructions to the agent's context. Every run is itself an event, so frontends show which hooks ran on which event and what each did: ok, context added, blocked with its reason, failed, timed out, running. | Through events only (INV-7). | none |

## Memory and context

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-7** | `Memory` | Project journal: add, search (lexical + semantic), list by category, tag and flag, link entries, close work items. Mind maps (a tree of what is known about a topic) and flowcharts (how something flows from a start to an end: ISO 5807 symbols, logic gates, labelled arrows) are separate stores with their own operations — create, change, read, draw ([ADR 0011](../decisions/0011-mind-maps-and-flowcharts.md)). A flowchart also keeps its **runs**: the live state of each pass of the process it is bound to and the ordered history of its events, listable, replayable and comparable ([ADR 0012](../decisions/0012-flowcharts-as-live-pipelines.md)). In step-debug mode a run that reaches a terminal block is saved to the project's SQLite under `.harness/debug/` and the chart is reset for the next cycle ([ADR 0013](../decisions/0013-step-debug-mode.md)). | Its own store; never sees secrets. | Local daemon (CON-20) |
| **IF-8** | `ContextStrategy` | Builds the context for a turn from the task, the session's recent events, and memory (specifications and constraints that bind the files in play, the task's mind map, related journal entries); compacts without losing what memory holds. | Reads memory and the session log; writes nothing but the context. | Journal- and map-aware compaction |
| **IF-9** | `SessionStore` | Persists the append-only session log (INV-5); lists, resumes, rewinds to event N, branches from event N. | Its own store. | Local files / browser storage |

## Proof

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-10** | `Verifier` | Given a task, its verification protocol and the specifications that bind it, runs or checks the protocol and returns pass / fail with the evidence, which goes into memory. | Runs tools through the loop like the agent, under `Policy`. | Protocol runner: commands and expected outcomes from the task's protocol |

## Reach

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-11** | `Frontend` | Presents the session (stream, tool calls, approvals, diffs, maps) and takes input. A window watching a live flowchart shows its current run in place: the active node (the one the latest callback came from), the path taken, failures, off-chart events. The session's mode (step-debug) is shown while it is on. Several may be attached at once. | Through events and the loop's input only. | Terminal (ratatui); browser (WebAssembly) |
| **IF-12** | `Transport` | Addresses sessions by name, delivers messages between them and to the user's channels (e.g. a messenger), across machines. | Authenticated per peer; carries no secrets of other plugins. | Local daemon; remote daemon over TLS |
| **IF-13** | `Trigger` | Routes an incoming callback (an event from outside: CI, a webhook, a script, another machine) by matching its label and payload fields, and performs the matched trigger's action: `message` (an inbound CALLBACK card in the bound session, the default), `skill` (start a run of the named skill with the event as input, in a new or a named session), `journal` (write a journal entry from the event; no agent turn), `flow` (move a run of a bound flowchart to a node or an arrow: live state in the watching windows, every event kept in the run's history; no agent turn — [ADR 0012](../decisions/0012-flowcharts-as-live-pipelines.md)). User-defined triggers add their own actions. Every callback gets an id and a result (which trigger matched, what it did) that the caller can read; the same delivery id is processed once. See [ADR 0009](../decisions/0009-callback-triggers.md). | Accepts callbacks only from authenticated callers (a token per caller, or the local socket of the user); the payload is data (INV-12). Starts work only through the loop, under `Policy`; writes memory only through `Memory`. | `message`, `skill`, `journal`, `flow` |
| **IF-14** | `Skill` | A named package of instructions, allowed tools and policy that sets an agent's mindset for a task; a run of a skill loads them into the context and limits the tools to the skill's set. Started by the user, by another skill, or by a `skill` trigger (IF-13). | Can narrow, never widen, the capabilities `Policy` allows. | Skills from the project and the user's config directory |

## Platform (INV-1, P-6)

| Id | Interface | Native | Browser |
|---|---|---|---|
| **IF-20** | `Fs` | the file system | in-memory or origin-private file system |
| **IF-21** | `Process` | child processes | not available — tools that need it are not offered |
| **IF-22** | `Net` | HTTP client | `fetch` |
| **IF-23** | `Clock` | system clock, timers | `performance`, timers |
| **IF-24** | `Storage` | key-value / SQLite | IndexedDB |
| **IF-25** | `Terminal` | the terminal | a terminal rendered in the page |
