# 0015 — Callback handlers are plugins

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-13, IF-15, IF-16, INV-12, INV-14; builds on [ADR 0002](0002-plugins-are-crates.md) (a plugin is a crate), [ADR 0009](0009-callback-triggers.md) (callback triggers) and [ADR 0014](0014-reliability-monitoring-mode.md) (conditions)

## Decision

Callbacks have custom types (IF-13: a callback's `type` is a harness type or any product string), so custom behaviour needs custom handlers. **One interface, `CallbackHandler` (IF-16), handles callbacks of every type:**

- the **standard types** are implemented through it and ship with the harness: `notification` (an inbound card in the bound session), `flowstate_update` (moves a run of a bound flowchart), a journal entry, a skill run — the actions of ADR 0009 and ADR 0012;
- **custom callbacks** are implemented through the same interface by the agent or the user: the handler of a product type, its firing policy (a `Condition`, IF-15: when its batch reaches the agent) and its response policies (Markdown files, CON-26).

A custom handler is a plugin like any other ([ADR 0002](0002-plugins-are-crates.md)): a crate in the local build. It is **plugged in through the TUI's settings**: a callbacks section lists the handlers in the build, and there the user turns one on, binds it to the types it handles, to its conditions and policy files, and to the session that receives it. The settings are written to the configuration; the TUI is the place to do it, not the only one (a configuration file stays valid).

The daemon routes an incoming callback by its `type` to the handler bound to it (IF-13); a type with no handler is recorded and reported back to the caller, never guessed.

## Boundaries

- A handler gets the callback as data (INV-12): it can act on it, but the payload cannot change handlers, bindings or policies.
- A handler acts only through the harness's interfaces — messages to the agent through the loop, memory through `Memory`, tools under `Policy` — like every plugin.
- Idempotency and batching (INV-14) hold for every handler, standard or custom; a handler cannot turn them off.

## Why

Product callbacks are as varied as products. A fixed set of actions would force every project into `message` and `journal`; one interface for the standard and the custom types keeps the core small and lets each project teach the harness its own events — the plugin-first principle applied to callbacks.

## Open

Whether a newly written handler needs a rebuild of the local binary (as ADR 0002 implies) or can be loaded without one (Q-24).
