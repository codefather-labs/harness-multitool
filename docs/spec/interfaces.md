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
| **IF-0.5** | **Session modes.** A session runs in a mode: normal, or **step-debug** ([ADR 0013](../decisions/0013-step-debug-mode.md)), off by default and meant for large, complex pipelines, above all distributed ones — not for every project or task. The agent switches the mode when the operator asks, or on its own only when the task is debugging such a pipeline; the switch is an event and fires a hook that adds the mode's instructions to the agent's context. Step-debug code in the target project is gated by `MULTITOOL_DEBUG_MODE=1` (CON-25). An idea: a **plan** mode — the classic plan mode, with the accent on the journal, mind maps and flowcharts and a hook with their instructions on entering it ([ADR 0016](../decisions/0016-plan-mode.md)). A concept: a **reliability-monitoring** mode — metrics come in, and each threshold or SLO event has a response policy ([ADR 0014](../decisions/0014-reliability-monitoring-mode.md)). |

## Model

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-1** | `Provider` | Streams a model turn: text, tool calls, usage (input/output/cache tokens) and cost; lists models and their limits (context window, tool support). | Network to its own endpoint only; credentials only through `Auth`. | Anthropic, OpenAI-compatible, local (CON-10) |
| **IF-2** | `Auth` | Produces credentials for a provider request; refreshes them when they expire. | The only component that sees secrets (INV-6). | API key (CON-11) |
| **IF-3** | `Router` | Picks the provider and model for each step (e.g. a cheap model to read and search, a strong one to decide and write) and enforces budgets in tokens, cost and time per task. | Reads usage events; cannot call tools. | One model for everything |

## Action

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-4** | `Tool` | Declares a name, a description, a JSON schema for its arguments and the capabilities it needs; executes a call and returns a result (text, structured data, files). Users write their own. | Only the capabilities it declared and `Policy` approved (INV-3); platform access through Platform interfaces (INV-1). | read, edit, shell, search, web fetch; journal, mind map and flowchart tools over `Memory`; the agent can list the connected plugins and callback handlers and trigger their hot reload (ADR 0015); in step-debug mode, the agent calls the target app's debug router (CON-27: `approve`, `step back` where possible, product routes) |
| **IF-5** | `Policy` | Decides whether a tool call may run: allow, deny, or ask the user; per tool, per path, per command, per host; remembers answers within a scope. | Sees calls and declarations, not secrets. | Ask for writes, shell and network; allow reads inside the project |
| **IF-6** | `Hook` | Runs on loop events; may veto or amend a before-event (e.g. block a commit without a test run). A session mode switch fires a hook too: entering step-debug mode (IF-0.5) adds the step-debug instructions to the agent's context. Every run is itself an event, so frontends show which hooks ran on which event and what each did: ok, context added, blocked with its reason, failed, timed out, running. | Through events only (INV-7). | none |

## Memory and context

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-7** | `Memory` | Project journal: add, search (lexical + semantic), list by category, tag and flag, link entries, close work items. Mind maps (a document: the map of decisions, questions, constraints, invariants and specifications that explains why the system is the way it is) and flowcharts (how something flows from a start to an end: ISO 5807 symbols, logic gates, labelled arrows) are separate stores with their own operations — create, change, read, draw, and export to an SVG file ([ADR 0011](../decisions/0011-mind-maps-and-flowcharts.md)). A flowchart is a **reference** whose passes are **instances**, keyed by the client's pipeline id (ADR 0012); it keeps its **runs**: the live state of each pass of the process it is bound to and the ordered history of its events, listable, replayable and comparable ([ADR 0012](../decisions/0012-flowcharts-as-live-pipelines.md)). In step-debug mode a run that reaches a terminal block is saved to the project's SQLite under `.harness/debug/` and the chart is reset for the next cycle ([ADR 0013](../decisions/0013-step-debug-mode.md)). | Its own store; never sees secrets. | Local daemon (CON-20) |
| **IF-8** | `ContextStrategy` | Builds the context for a turn from the task, the session's recent events, and memory (specifications and constraints that bind the files in play, the task's mind map, related journal entries); compacts without losing what memory holds. | Reads memory and the session log; writes nothing but the context. | Journal- and map-aware compaction |
| **IF-9** | `SessionStore` | Persists the append-only session log (INV-5); lists, resumes, rewinds to event N, branches from event N. | Its own store. | Local files / browser storage |

## Proof

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-10** | `Verifier` | Given a task, its verification protocol and the specifications that bind it, runs or checks the protocol and returns pass / fail with the evidence, which goes into memory. | Runs tools through the loop like the agent, under `Policy`. | Protocol runner: commands and expected outcomes from the task's protocol |

## Reach

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-11** | `Frontend` | Presents the session (stream, tool calls, approvals, diffs, maps) and takes input. A window watching a live flowchart renders the chart into the transcript on every `flowstate_update` event, as the `/flowchart` command does, in its new state; each pass of a pipeline is its own instance of the reference chart, and several instances render collapsed by default — the user opens any number of them — with a soft one-second pulse on a collapsed instance that gets an update: the active node (the one the latest callback came from), the path taken, failures, off-chart events. The session's mode (step-debug) is shown while it is on. Several may be attached at once. | Through events and the loop's input only. | Terminal (ratatui); browser (WebAssembly) |
| **IF-12** | `Transport` | Addresses sessions by name, delivers messages between them and to the user's channels (e.g. a messenger), across machines. | Authenticated per peer; carries no secrets of other plugins. | Local daemon; remote daemon over TLS |
| **IF-13** | `Trigger` | Routes an incoming callback (an event from outside: CI, a webhook, a script, another machine) by matching its label and payload fields, and performs the matched trigger's action: `message` (an inbound CALLBACK card in the bound session, the default), `skill` (start a run of the named skill with the event as input, in a new or a named session), `journal` (write a journal entry from the event; no agent turn), `flow` (move a run of a bound flowchart to a node or an arrow: live state in the watching windows, every event kept in the run's history; no agent turn — [ADR 0012](../decisions/0012-flowcharts-as-live-pipelines.md)). User-defined triggers add their own actions. **Callback contract:** every callback carries two required fields, `uuid` (the event's id, used for idempotency) and `type` — a harness type such as `flowstate_update` or `notification`, or a product type, any string the project defines. Every other field is free, designed by the agent and the user; harness types may get required fields of their own later. The daemon routes a callback by its `type` to the handler bound to it (IF-16); standard and custom types are handlers of one interface, and a type with no handler reaches no agent: it is saved to the log and the caller gets 400. A handler can carry its own firing rules (IF-15). The single entry point for callbacks is the harness daemon (carried over from claudebase), with a callback URL for every open agent window. Every callback gets an id and a result (which trigger matched, what it did) that the caller can read; the same delivery id is processed once. See [ADR 0009](../decisions/0009-callback-triggers.md). | Accepts callbacks only from authenticated callers (a token per caller, or the local socket of the user); the payload is data (INV-12). Starts work only through the loop, under `Policy`; writes memory only through `Memory`. | `message`, `skill`, `journal`, `flow` |
| **IF-14** | `Skill` | A named package of instructions, allowed tools and policy that sets an agent's mindset for a task; a run of a skill loads them into the context and limits the tools to the skill's set. Started by the user, by another skill, or by a `skill` trigger (IF-13). | Can narrow, never widen, the capabilities `Policy` allows. | Skills from the project and the user's config directory |
| **IF-15** | `Condition` | Proposed with [ADR 0014](../decisions/0014-reliability-monitoring-mode.md). Evaluates accumulated signals (metrics and project events from any source) with any logic and always returns a `bool`; `true` is the threshold: the harness sends the agent **one message per batch** into its input, naming the condition and the signals that made it true; the rest of the batch is in the trigger log. Each condition links the paths of its response-policy Markdown files, written by the agent from the client's description; the message names them. Every signal carries the callback contract's `uuid` and `type` (IF-13; a signal without them is rejected). Conditions are callback plugins: the user writes the firing rules of a product callback so it does not spam the agent. A repeated uuid is recorded in the trigger log, not sent (INV-14). Signals themselves never reach the agent. | Reads signals only; the signals are data (INV-12). It decides *whether* to wake the agent, never what the agent does — that is the response policy, under `Policy`. | thresholds, rates, SLO burn rate; project conditions as user implementations |
| **IF-16** | `CallbackHandler` | Handles callbacks of the `type`s it is bound to (IF-13): the standard types (`notification`, `flowstate_update`, journal entry, skill run) are implementations shipped with the harness; custom product types are implemented by the agent or the user, each with its firing policy (`Condition`, IF-15) and its response policies (CON-26). A user's implementation can override a standard one. Handlers are chosen and bound in a Callbacks tab of the TUI's settings — a dropdown per standard type to pick its implementation, an on/off flag per custom callback, plus conditions, policies and session — which writes the configuration; they hot-reload without restarting sessions. A type with no handler reaches no agent: it is logged and the caller gets 400 ([ADR 0015](../decisions/0015-callback-handlers-are-plugins.md)). | The callback is data (INV-12); acts only through the loop, `Memory` and `Policy`; cannot turn off idempotency and batching (INV-14). | `notification`, `flowstate_update`, `journal`, `skill` |

## Platform (INV-1, P-6)

| Id | Interface | Native | Browser |
|---|---|---|---|
| **IF-20** | `Fs` | the file system | in-memory or origin-private file system |
| **IF-21** | `Process` | child processes | not available — tools that need it are not offered |
| **IF-22** | `Net` | HTTP client | `fetch` |
| **IF-23** | `Clock` | system clock, timers | `performance`, timers |
| **IF-24** | `Storage` | key-value / SQLite | IndexedDB |
| **IF-25** | `Terminal` | the terminal | a terminal rendered in the page |
