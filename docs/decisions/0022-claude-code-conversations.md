# 0022 — Moving a Claude Code conversation onto the harness

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-17, IF-18, CON-69, CON-70, CON-71, CON-72; builds on [ADR 0020](0020-claude-code-delegate.md)

## Decision

A conversation can move onto the harness and go on there, with its whole context.

The context can only go on where it lives, in Claude Code, and the harness never holds a Claude subscription's tokens (CON-56, CON-57). So the conversation is not converted for another model. It goes on through the delegate, and the harness gets a copy for the screen.

- `multitool import claude-code` lists the conversations of this directory; with an id it stores one as a session of this project. It only reads the original.
- The first message of that session runs `claude --resume <id> --fork-session`. The original stays whole and can be returned to; from then on the fork's own id is resumed.
- The screen's copy is the conversation as it stood: the chain from the last line back, by parent, across compactions. A branch that was rewound, a subagent's lines and Claude Code's own wrapping are out. Tool results are kept to 4 KiB and images are out: the whole stays in Claude Code's record.
- Claude Code's transcript format is its own and undocumented. The reader passes over what it does not know and counts what it cannot read.

## What goes with it

A delegate is isolated by default. A conversation that moves needs what it has been working with, so a provider row can opt in:

```toml
[session.providers.claude-code]
kind = "claude-code"
environment = "user"   # the user's own settings, hooks, servers and environment
permissions = "skip"   # no permission prompts, as the user may run Claude Code themselves
```

`permissions = "skip"` is only ever the user's own setting. Without it, tools that need approval are denied until the harness answers prompts itself.

## Limits

- It opens only in the directory it began in: Claude Code resumes by directory.
- Messages reaching a running Claude Code window from outside do not arrive in `claude -p`.
- An imported session is for the delegate: its history is not sent to another provider.
