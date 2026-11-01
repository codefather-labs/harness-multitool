# 0009 — Callbacks are triggers

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-13, IF-14, INV-12, CON-23, Q-9, Q-10

## Decision

External systems (CI, deploy scripts, webhooks, monitors, other machines) send events straight into the harness. A callback is not only a message: by design each callback is routed by a **trigger** that says what the event does. Users define their own triggers; three kinds ship:

| Kind | What the event does | Agent turn? |
|---|---|---|
| `notification` | Delivered into the agent's input (stdin) and shown in the bound session as an inbound CALLBACK card. (Called `message` until 2026-11-01.) | when the session next runs |
| `skill` | Starts a run of a named skill with the event as its input: the agent takes the callback up **with the skill's mindset** (its instructions, tools and policy), in a new session or in a named one. | yes, started by the event |
| `journal` | Writes the event to the project journal as an entry (category, tags, flags from the trigger, the body from the payload). Nothing runs. | never |

A trigger matches callbacks by label and payload fields, and names its action and its target (a new session, a named session, the active one). Triggers are configuration, versioned with the project; they appear in the TUI like any other event: a `skill` run as a new tab or a CALLBACK card with the skill it started, a `journal` write as a JOURNAL event (journal#136).

## Why

- The harness already has the pieces: a transport that addresses sessions (IF-12), a journal (IF-7), an agent loop that runs under policy (IF-0.1, IF-5). Triggers connect them, so automation needs no glue scripts that start an agent by hand.
- A skill gives an automated run the right mindset and limits: a `fix-ci` skill brings its instructions and only the tools it needs, instead of a general agent improvising.
- Not every event deserves a model turn. Deploys, releases and alerts often only need to be on record; a `journal` trigger records them for free and the next session finds them in memory.

## Rejected

- **Callbacks only as messages** (the state before this decision): every event costs an agent turn or is lost; nothing can start work on its own.
- **A general workflow engine** (conditions, loops, branches in the trigger config): disproportionate; a skill already holds the logic, the trigger only routes.
- **Letting the payload pick the skill or the tools:** the caller would control what runs. The trigger, written by the user, decides; the payload is data (INV-12).

## Consequences

- New interfaces: `Trigger` (IF-13), `Skill` (IF-14). Delivery stays with `Transport` (IF-12).
- New invariant INV-12: a callback's payload is data, never instructions or permissions.
- Open questions: the trigger configuration syntax and payload templating (Q-9); how a skill run reports back to the caller (Q-10).

**Amended by [ADR 0015](0015-callback-handlers-are-plugins.md):** every callback carries a `uuid` and a `type`, and the daemon routes it by `type` to a `CallbackHandler`; the kinds above are the harness types (the owner, 2026-11-01): `message` is removed — it is `notification`; `journal` only writes to the journal; `skill` stays as a concept and may be dropped; the `flow` kind of [ADR 0012](0012-flowcharts-as-live-pipelines.md) is renamed `flowstate_update`. There is no default any more: a callback whose type has no handler reaches no agent, is logged, and the caller gets 400.
