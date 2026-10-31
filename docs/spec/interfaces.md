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

## Model

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-1** | `Provider` | Streams a model turn: text, tool calls, usage (input/output/cache tokens) and cost; lists models and their limits (context window, tool support). | Network to its own endpoint only; credentials only through `Auth`. | Anthropic, OpenAI-compatible, local (CON-10) |
| **IF-2** | `Auth` | Produces credentials for a provider request; refreshes them when they expire. | The only component that sees secrets (INV-6). | API key (CON-11) |
| **IF-3** | `Router` | Picks the provider and model for each step (e.g. a cheap model to read and search, a strong one to decide and write) and enforces budgets in tokens, cost and time per task. | Reads usage events; cannot call tools. | One model for everything |

## Action

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-4** | `Tool` | Declares a name, a description, a JSON schema for its arguments and the capabilities it needs; executes a call and returns a result (text, structured data, files). Users write their own. | Only the capabilities it declared and `Policy` approved (INV-3); platform access through Platform interfaces (INV-1). | read, edit, shell, search, web fetch |
| **IF-5** | `Policy` | Decides whether a tool call may run: allow, deny, or ask the user; per tool, per path, per command, per host; remembers answers within a scope. | Sees calls and declarations, not secrets. | Ask for writes, shell and network; allow reads inside the project |
| **IF-6** | `Hook` | Runs on loop events; may veto or amend a before-event (e.g. block a commit without a test run). Every run is itself an event, so frontends show which hooks ran on which event and what each did: ok, context added, blocked with its reason, failed, timed out, running. | Through events only (INV-7). | none |

## Memory and context

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-7** | `Memory` | Project journal: add, search (lexical + semantic), list by category, tag and flag, link entries, close work items; mind maps and flowcharts: create, change, read, draw. | Its own store; never sees secrets. | Local daemon (CON-20) |
| **IF-8** | `ContextStrategy` | Builds the context for a turn from the task, the session's recent events, and memory (specifications and constraints that bind the files in play, the task's mind map, related journal entries); compacts without losing what memory holds. | Reads memory and the session log; writes nothing but the context. | Journal- and map-aware compaction |
| **IF-9** | `SessionStore` | Persists the append-only session log (INV-5); lists, resumes, rewinds to event N, branches from event N. | Its own store. | Local files / browser storage |

## Proof

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-10** | `Verifier` | Given a task, its verification protocol and the specifications that bind it, runs or checks the protocol and returns pass / fail with the evidence, which goes into memory. | Runs tools through the loop like the agent, under `Policy`. | Protocol runner: commands and expected outcomes from the task's protocol |

## Reach

| Id | Interface | Behaviour | Boundary | Default |
|---|---|---|---|---|
| **IF-11** | `Frontend` | Presents the session (stream, tool calls, approvals, diffs, maps) and takes input. Several may be attached at once. | Through events and the loop's input only. | Terminal (ratatui); browser (WebAssembly) |
| **IF-12** | `Transport` | Addresses sessions by name, delivers messages between them and to the user's channels (e.g. a messenger), across machines. | Authenticated per peer; carries no secrets of other plugins. | Local daemon; remote daemon over TLS |
| **IF-13** | `Trigger` | Routes an incoming callback (an event from outside: CI, a webhook, a script, another machine) by matching its label and payload fields, and performs the matched trigger's action: `message` (an inbound CALLBACK card in the bound session, the default), `skill` (start a run of the named skill with the event as input, in a new or a named session), `journal` (write a journal entry from the event; no agent turn). User-defined triggers add their own actions. Every callback gets an id and a result (which trigger matched, what it did) that the caller can read; the same delivery id is processed once. See [ADR 0009](../decisions/0009-callback-triggers.md). | Accepts callbacks only from authenticated callers (a token per caller, or the local socket of the user); the payload is data (INV-12). Starts work only through the loop, under `Policy`; writes memory only through `Memory`. | `message`, `skill`, `journal` |
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
