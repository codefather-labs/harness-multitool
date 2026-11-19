# Move a conversation to another adapter

One conversation goes from Claude Code to Codex, then to a local Ollama model, then back to Claude Code.
- Each move writes the conversation in the form the target reads, and a toast shows the move's progress.
- Each adapter goes on from everything said before: Codex sums up what was done on Claude Code, and Ollama records its decision after the entry Claude Code wrote.
- Moving back, the daemon compares the form with the log and writes only the turns it lacks.

<!-- how-to: move-conversation -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/move-conversation) this page plays it live (CON-187).

## In the real harness

- Settings > Provider, choose another adapter, Ctrl+S: every open conversation of the window moves onto it.
- A conversation is the harness's own log; Claude Code's transcript, Codex's session files and an API's request history are its forms, kept in step before each step (INV-17, INV-20).
- `claude --resume <id>` opens the same conversation in Claude Code itself.

## See also

[Adapters: one conversation in every form](../adapters.md#one-conversation-in-every-form), ADR 0025, CON-134, CON-136, CON-138, CON-144.
