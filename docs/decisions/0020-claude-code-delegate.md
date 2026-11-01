# 0020 — Claude Code as a delegate agent

- **Status:** proposed · **Raised by:** the project owner, 2026-11-01
- **Specs:** IF-2, IF-17, INV-4, INV-5, INV-6, INV-9, CON-56, CON-57, CON-58, CON-59, CON-60, CON-61, Q-33, Q-34, Q-35; follows [ADR 0019](0019-vendor-documented-sign-in.md)

## Decision

A session can run its turns through the **unmodified `claude` binary**, signed in by the user through Anthropic's own flow. Claude Code stays Claude Code: its system prompt and its tools are its own, and the harness adds to them and shows what it does. The harness never reads, stores or passes its credentials, so this is not an `Auth` adapter: it is a second `AgentBackend` (IF-17).

Anthropic's [terms](https://code.claude.com/docs/en/legal-and-compliance) do not prevent an end user signing in to the unmodified binary. They also say subscription sign-in is meant for ordinary use of Claude Code, that developers of products should use API keys and may not route requests through plan credentials on behalf of their users, and that running Claude Code in a product takes their Commercial Terms. So this stays **out of the default build**, behind a cargo feature, until Anthropic answers whether it may ship (Q-33). This record is not legal advice.

**Not this:** Claude Code as a hidden model behind the harness's own tool loop, with its tools switched off and its prompt replaced. That reads as a way round the terms, and external tools only reach it through MCP in any case.

## Two modes

| Mode | How | Fits |
|---|---|---|
| `oneshot` (first) | `claude -p` per message, the prompt on stdin, `--resume` with the stored session id; nothing runs between messages (CON-58) | Reliability, a hot reload, many idle tabs |
| `session` | One process per tab with `--input-format stream-json`, one result per message; ended after 10 idle minutes and at a hot reload, back with `--resume` (CON-59) | Long work, if its first-event time turns out better |

Which is the default is decided by measuring (Q-34).

## What the harness keeps

The event log, so the transcript, a resume and the status line work unchanged; permissions, through its own prompt by `--permission-prompt-tool`, where no answer is a denial; stop; plan mode as `--permission-mode plan`; the `Verifier` for a task's done (INV-4). Its memory and debug tools reach Claude Code through `multitool mcp` on stdio. The user's own hooks and servers should not run in the nested process (CON-61): `--bare` cannot be used for that, because it answers "Not logged in" (checked 2026-11-01, version 2.1.316).

## What is open

Q-33, whether it may ship; Q-34, the default mode; Q-35, long sessions: who compacts and whether the log stays in step, memory when idle, a dropped pipe, limits.
