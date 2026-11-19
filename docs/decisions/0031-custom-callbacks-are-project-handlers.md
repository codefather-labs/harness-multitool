# 0031 — Custom callbacks are project handlers

- **Status:** accepted; amends [ADR 0015](0015-callback-handlers-are-plugins.md) for custom types · **Decided by:** the project owner, 2026-11-19 (journal#1364, journal#1366, journal#1370, journal#1372, journal#1373)
- **Specs:** CON-183, IF-13, IF-16, CON-164, CON-166 · **Docs:** [docs/callbacks.md](../callbacks.md)

## Context

ADR 0015 made a custom callback handler a plugin crate linked into the multitool binary. No such crate was ever built (journal#101). A project that wants its own callback type, such as `deploy_done`, should not have to rebuild the harness.

## Decision

A custom callback belongs to the **project**: `.harness/callbacks/<name>/` holds a `config.toml` and a handler.

- **The handler is a Rust program**, built on the harness's contract crate `harness-callback` (crates/callback-sdk). `multitool callback new` makes one from a template and `multitool callback build` builds it. Scripts are refused: there is one way to run a handler, with no interpreters or environments to manage for each language. The handler's own logic may call anything else.
- **The daemon stands at the start, as for every callback.** A custom callback comes to a conversation's URL with a uuid and a type, like a default one. The daemon keeps a registry, conversation → type → handler, in hash tables, so a callback finds its handler in O(1). Standard types stay native.
- **The handler stands between the daemon and the conversation.** The daemon answers 202 and starts the handler without waiting for it. The handler gets the callback on stdin, its args, and the conversation and a reply URL in its environment. It POSTs its results to that URL: a message, a journal entry, or nothing. The daemon delivers them to that conversation alone.
- **Trust.** The user approves a handler in Settings > Callbacks, and the approval pins it by its SHA-256. Each conversation is subscribed by the user. A handler runs in a systemd scope of its own, with a timeout and an output cap.

## Rejected

- **Results on stdout.** That would make the daemon wait for the handler, and a handler could not answer more than once. It was the owner's word that the daemon does not wait.
- **Any executable, in any language.** The owner chose Rust only, so that the harness does not grow a runner for each language.
- **One pool of callbacks for the whole project.** The owner chose a pool per conversation, so that one agent's callbacks do not wake another.
