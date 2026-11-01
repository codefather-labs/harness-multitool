# 0015 — Callback handlers are plugins

- **Status:** accepted; hot reload amends [ADR 0002](0002-plugins-are-crates.md), mechanism open (Q-24) · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-13, IF-15, IF-16, INV-12, INV-14; builds on [ADR 0002](0002-plugins-are-crates.md) (a plugin is a crate), [ADR 0009](0009-callback-triggers.md) (callback triggers) and [ADR 0014](0014-reliability-monitoring-mode.md) (conditions)

## Decision

Callbacks have custom types (IF-13: a callback's `type` is a harness type or any product string), so custom behaviour needs custom handlers. **One interface, `CallbackHandler` (IF-16), handles callbacks of every type:**

- the **standard types** are implemented through it and ship with the harness: `notification` (an inbound card in the bound session), `flowstate_update` (moves a run of a bound flowchart), a journal entry, a skill run — the actions of ADR 0009 and ADR 0012;
- **custom callbacks** are implemented through the same interface by the agent or the user: the handler of a product type, its firing policy (a `Condition`, IF-15: when its batch reaches the agent) and its response policies (Markdown files, CON-26).

A custom handler is a plugin like any other ([ADR 0002](0002-plugins-are-crates.md)): a crate in the local build. It is **plugged in through the TUI's settings** (the Callbacks tab below), which lists the handlers in the build. The settings are written to the configuration; the TUI is the place to do it, not the only one (a configuration file stays valid).

**Standard handlers can be overridden.** A user's implementation can replace the shipped handler of a standard type.

**A Callbacks tab in the settings, by design.** Callbacks get a settings tab of their own in the TUI:
- each **standard** type has a dropdown that picks its implementation: the shipped one or an override;
- each **custom** callback has an on/off flag;
- the same tab binds conditions, policy files and the receiving session.

**Hot reload.** Any plugin and any callback handler can be connected, replaced or disconnected **without restarting sessions**. The agent can trigger the reload itself, and can list the plugins and callback handlers that are connected (IF-4). How the reload works with plugins that are crates (ADR 0002) is open (Q-24).

**A type with no handler.** The daemon routes an incoming callback by its `type` to the handler bound to it (IF-13). If no handler is bound to the type, nothing reaches the agent: the callback is saved to the log, and the caller gets **400**. The harness never guesses where to send it.

## Boundaries

- A callback is only data (INV-12): a handler can act on it, but the payload cannot change handlers, bindings or policies. Some types trigger a harness action — `flowstate_update` changes a flowchart's state — but only because their handler does it, never because the payload says so.
- A handler acts only through the harness's interfaces — messages to the agent through the loop, memory through `Memory`, tools under `Policy` — like every plugin.
- Idempotency and batching (INV-14) hold for every handler, standard or custom; a handler cannot turn them off.

## Why

Product callbacks are as varied as products. A fixed set of actions would force every project into `message` and `journal`; one interface for the standard and the custom types keeps the core small and lets each project teach the harness its own events — the plugin-first principle applied to callbacks.

## Open

The hot-reload mechanism, and the amendment of ADR 0002 it needs (Q-24).
