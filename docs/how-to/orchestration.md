# Orchestrate many agents

A master writes three tasks into the journal and starts an agent on each:
- `docs-writer` on Codex;
- `tester` on Claude Code;
- `changelog` on a local Ollama model.

Each appears as an (S) tab of the master's group and works in its own conversation; the window visits each one. Each writes its proof to the journal and asks the master to close its task. The master reads its work, closes the entry and ends its session.

<!-- how-to: orchestration -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/orchestration) this page plays it live (CON-187).

## In the real harness

- Ask an agent to split the work, or put it in orchestration mode (`mode__orchestrate`).
- The master writes each task as an open journal entry, then `mode__agent_spawn` with `nick`, `adapter` and `task`.
- A slave reports with `agent_send`; only the master closes its task, after `mode__agent_read`, and ends it with `mode__agent_close`.
- The group's tabs are marked (M) and (S); the status line's MODE says `orchestrate` and `slave`.

## See also

[Orchestration](../orchestration.md), [Agent workflow, in an orchestra](../agent-workflow.md#6-in-an-orchestra), CON-100, CON-108, CON-109.
