# 0024 — Conversations live in the daemon

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** [conversations.md](../spec/conversations.md), INV-15..19, IF-19, CON-75, CON-76, CON-77; builds on [ADR 0023](0023-the-log-is-the-conversation.md)

## Decision

A conversation always lives in the daemon, and every message in and out goes through it. A frontend sends a message to the daemon. The daemon, knowing the adapter connected to this conversation, brings that provider's own form up to date and calls it. The reply is stored in the daemon, which brings it to the frontend. So there is one consistent conversation, which can be switched to any adapter at any time, and which does not depend on a window being open.

A session's engine runs in the daemon: its log, its tools and its calls to a provider. A frontend is a client with the same operations and hears the same events. A window that goes away leaves its runs to finish; a prompt nobody can answer is a denial.

With Claude Code, the daemon brings the transcript up to all earlier history and sends the new message as the prompt, and Claude Code records it itself. It refuses to go on from a message already in the file with no prompt, and writing it first would send it twice.

## What stays in the frontend

Only a scripted run, `multitool run` for now, and a window that cannot have a daemon run sessions of their own. A daemon of another build is replaced when it is idle; if it is running conversations, the window works on its own.

## What this asks of a window

A window's environment travels with its operations and stays in the daemon's memory for that window's sessions.
