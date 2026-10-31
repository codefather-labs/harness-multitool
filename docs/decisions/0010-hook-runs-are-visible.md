# 0010 — Hook runs are visible

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-6, IF-0.2, IF-11

## Decision

Every run of a hook is an event on the bus, and frontends show it: on which loop event the hooks ran (session start, compaction, before or after a tool call, prompt submit, stop, ...), which scripts ran, and what each did — ok, added context (with the lines it added), blocked the event (with the reason), failed (exit code, stderr tail), timed out, still running — with its duration. The terminal UI draws one HOOK card per event ([docs/tui.md](../tui.md#hooks)).

## Why

Hooks change what the agent sees and may veto what it does. Invisible, they make the agent's behaviour hard to explain: a context line that came from a hook, a tool call that silently did not happen. The owner asked to see, for example, which scripts fire at session start and at compaction.

## Rejected

- **Show only failures and vetoes:** a hook that adds context changes the session as much as one that blocks; the owner wants to see every run.
- **A log file instead of the transcript:** the reader of the session would have to look elsewhere to learn why the agent knew or did something.
