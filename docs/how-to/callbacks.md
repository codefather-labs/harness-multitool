# Callbacks by design: standard and your own

Callbacks are part of the harness's design, not an add-on. Every conversation has its own URL, served by the daemon, and any app can wake the agent through it. The agent gives its own callback URL, and a notification from CI then wakes it.

A project's own callback, `deploy-done`, is turned on with one Enter in Settings > Callbacks. That approves its handler as it is now and subscribes this conversation. The handler's result then reaches the agent, which records the deploy.

<!-- how-to: callbacks -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/callbacks) this page plays it live (CON-187).

## In the real harness

- `callback_url` gives the URL. An app POSTs JSON with a unique `uuid` and a `type`:
  - `notification` reaches the agent as a message;
  - `journal` writes an entry;
  - a project's own type goes to its handler.
- `multitool callback new deploy-done --types deploy_done,deploy_failed`, then `multitool callback build deploy-done`. A handler is a Rust program on the `harness-callback` crate.
- Turning it on is the user's: Enter on its row in Settings > Callbacks. A changed handler must be turned on again.

## See also

[Callbacks](../callbacks.md), CON-164, CON-165, CON-183.
