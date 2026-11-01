# 0025 — One conversation in every form

- **Status:** accepted · **Decided by:** the project owner, 2026-11-02
- **Specs:** [conversations.md](../spec/conversations.md), INV-20, INV-21, IF-18, CON-71, CON-74, CON-77, CON-78; replaces the fork of [ADR 0022](0022-claude-code-conversations.md)

## Decision

The daemon's log and a provider's form of a conversation are consistent at every moment, by design. The owner's requirement is to move a running conversation onto the harness and back to the provider's own program at any time, with the conversation the same on both sides.

So a conversation is linked, never copied. It goes on under the id it has. Before each step the daemon pulls what the form gained into the log and pushes what the log gained to the form; a provider's own program writes its turns itself, and only its position is recorded.

## Why not the fork

A fork at the first message kept the original safe by making a second conversation: the original never saw the harness's turns, and what happened to the original after the import never reached the harness. That is two conversations, which is what the owner ruled out.

## What makes appending safe

Only appending: every byte of a form stays as it was, so nothing the provider wrote can be lost. Measured on Claude Code 2.1.316: `claude -p --resume <id>` keeps the id and the file, and lines appended on the chain with a text-only assistant message are read as history by the next resume. The format is undocumented and can change with a version; a form that cannot be written or read is an error on screen.

## What this costs

Two programs cannot write one conversation at once. The harness refuses to send while the provider's program holds it (CON-78). It cannot prevent the opposite order, and a resume started in the provider's program during a harness run makes a branch; the file keeps both.

A form is created only when a session first goes on through its provider. A session that has only ever been on other providers has no Claude Code form to return to until then.
