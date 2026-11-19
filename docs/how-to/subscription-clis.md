# Orchestrate subscription CLIs through one daemon

A lead agent on Claude Code splits a feature between three CLI sessions: two of Claude Code and one of Codex. Each is the vendor's unmodified CLI, signed in to the user's own subscription. All four conversations are hosted by the one daemon of the machine and share one project journal:
- `qa` starts first and finds no format for `--json` in the journal, so it asks the lead;
- `api` decides the format and writes it into the journal;
- `cli-docs`, on Codex, finds that decision there and documents exactly it;
- the lead tells `qa` where the decision is, agent to agent;
- each session reports its proof; the lead checks it, closes the task and ends the session.

<!-- how-to: subscription-clis -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/subscription-clis) this page plays it live (CON-187).

## In the real harness

- Sign in Claude Code and Codex once per profile, each with its own program: Settings > Provider, or `multitool login claude-code` and `multitool login codex`. `mode__agent_models` lists what is signed in, and a spawn may use only that.
- The lead writes each task as an open journal entry, then `mode__agent_spawn` with `nick`, `adapter` (`claude-code` or `codex`) and `task`. Each session is the unmodified `claude -p` or `codex exec --json`, isolated, in a systemd scope of its own (CON-108, CON-166).
- One daemon hosts every conversation of the machine, so every session reads and writes the same journal and maps (`journal_search`, `journal_add`).
- A slave writes only to its lead with `agent_send`; the lead writes to any of its slaves. Only the lead closes a task, after `mode__agent_read`.
- Each user's plan limits are their own; the harness pays for, resells and routes no one's usage ([Delegates: terms](../delegates.md#terms-and-subscriptions)).

## See also

[Orchestration](../orchestration.md), [Delegates](../delegates.md), [Daemon](../daemon.md), [Journal](../journal.md), CON-57, CON-100, CON-108, CON-109, CON-166.
