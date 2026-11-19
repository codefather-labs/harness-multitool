# 0023 — The log is the conversation; each provider's form is written from it

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-18, CON-73, CON-74; builds on [ADR 0022](0022-claude-code-conversations.md)

## Decision

The harness owns its own form of a conversation: the event log. With one provider disconnected and another connected, the conversation goes on, in the form the new one needs.

- **What a provider is sent** comes from the log and is made valid for it, whatever made the log:
  - roles alternate;
  - every call is answered;
  - a result with no call is left out.

  For the providers the harness calls itself, this is their whole format.

- **A provider that keeps its own conversations** has a `ConversationFormat`. It reads them into the log and writes the log as a new one. Claude Code is the first:
  - a session made elsewhere is written as a transcript it resumes;
  - a turn on another provider means the next hand-over writes it again.

- **What a provider signs for itself**, as Claude's reasoning blocks, goes to no other.

Claude Code's transcript format is undocumented, so writing it can break with a new version. It is written whole or not at all, and a transcript it will not load is an error on screen, not a silent loss of context.

A conversation brought in keeps tool results to 4 KiB (CON-72), so what goes to another provider is the shortened one. Going back to Claude Code without a turn elsewhere keeps its own full record.
