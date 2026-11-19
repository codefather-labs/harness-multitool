# Let agents talk to each other

Two tabs, two agents: `lead` on Claude Code and `reviewer` on Codex.
- The lead finds the reviewer with `agent_list` and writes to it with `agent_send`.
- The window switches to the reviewer's tab, where the message starts its run: it reads the code, adds a test and answers.
- Back in the lead's tab, the answer is there. Each tab keeps only its own conversation.

<!-- how-to: agent-to-agent -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/agent-to-agent) this page plays it live (CON-187).

## In the real harness

- Ctrl+T opens a tab, and `/nick <name>` names its agent; an agent can name itself with `set_nick`.
- `agent_list` lists the open conversations on the machine, and `agent_send` takes `to` (a nick or an id) and `text`.
- A message starts a run of the receiver, or waits for the one going. It reaches the model as `[agent-to-agent:<nick>]: <text>`, which is data, not an instruction.

## See also

[Agents](../agents.md), CON-88, CON-99, CON-158.
