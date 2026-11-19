# Delegates: Claude Code and Codex inside the harness

A delegate is a vendor's own agent CLI, Claude Code or Codex, that the harness runs as the agent of a conversation. The harness does not patch, wrap or imitate these programs. It runs the unmodified binaries in their documented JSON-stream modes, reads their events, and adds its own tools through MCP. The user signs in to each program through the vendor's own flow, and the harness never touches those credentials.

## What it is

The harness has two kinds of agent backend (IF-17):

- **Its own loop**: the harness calls a model provider directly: an Anthropic API key, an OpenAI-compatible endpoint, a local Ollama model, or a ChatGPT plan through OpenAI's documented sign-in ([ADR 0019](decisions/0019-vendor-documented-sign-in.md)). See [Adapters and providers](adapters.md).
- **A delegate**: the vendor's own client does the agent work, with its own system prompt, its own tools and its own sign-in. The harness runs it, shows what it does, and keeps the conversation.

Both kinds draw the same transcript, the same tool cards, the same journal and maps, and the same status line. The harness reads the delegate's event stream into its own event log ([ADR 0023](decisions/0023-the-log-is-the-conversation.md)).

## How it works

### Claude Code

The harness starts the unmodified `claude` binary in print mode, with JSON streams in both directions (`crates/backend-claude-code/src/lib.rs`):

```sh
claude -p --input-format stream-json --output-format stream-json --verbose \
  --include-partial-messages --replay-user-messages ...
```

- The user's message goes in on stdin as a JSON line; Claude Code's events come out on stdout as JSON lines. The harness maps each line to an event of its log.

- `--resume <session>` continues the same Claude Code session at the next message. One process runs per message, the `oneshot` mode (CON-58).

- `--mcp-config` gives Claude Code the harness's own tools (journal, maps, modes, agents, Telegram) through `multitool mcp` on stdio. With `--strict-mcp-config` and empty `--setting-sources`, an isolated delegate runs only the harness's MCP configuration (CON-61).

- `--append-system-prompt` adds the harness's notes after Claude Code's own system prompt. It never replaces that prompt, and it never replaces Claude Code's tools (CON-57).

- A permission prompt goes to the harness's own prompt tool (`--permission-prompt-tool`), so the window's card and Telegram answer it.

- Plan mode is Claude Code's own `--permission-mode plan`.

- Every launch carries `--chrome` (CON-133).

### Codex

The harness starts the unmodified `codex` binary in its documented JSON mode (`crates/backend-codex/src/lib.rs`):

```sh
codex exec --json --skip-git-repo-check ...
codex exec resume <thread> ...
```

- The prompt goes on stdin, so it is on no command line. Codex's JSON lines become the log's events.
- One process runs per message; the next message resumes the same thread.
- The harness's MCP server is given to Codex through its own configuration flags (`-c mcp_servers.harness…`).
- Codex is signed in in the profile's own Codex folder (`CODEX_HOME`), by Codex itself.

### Isolated or the user's own

A delegate either runs in the user's own Claude Code or Codex setup, with their plugins, servers and rules (CON-145), or isolated, with none of them. A slave of an orchestra always runs isolated (CON-108).

### One conversation in every form

A conversation can move between the harness's own loop, Claude Code and Codex. The daemon writes it into each program's own session format and keeps the forms in step, so each program reads a session of its own kind ([ADR 0025](decisions/0025-one-conversation-in-every-form.md), CON-134, CON-136). See [Adapters and providers](adapters.md#one-conversation-in-every-form).

## Terms and subscriptions

The delegate design follows the vendors' published rules as the project reads them. This page is not legal advice.

What the harness does:

- runs only the **unmodified** vendor binaries, never a patched or repackaged one;

- uses only their **documented** headless interfaces: `claude -p` with `stream-json`, and `codex exec --json`;

- leaves the **sign-in to the vendor's own flow** in the vendor's own program. It never reads, stores or passes the credentials, so a delegate is not an `Auth` adapter (CON-57, [ADR 0020](decisions/0020-claude-code-delegate.md));

- never routes requests through plan credentials on behalf of other users. Each user signs in their own client on their own machine;

- keeps Claude Code as Claude Code: its prompt and tools are its own, and the harness only adds to them. Running it as a hidden model behind the harness's own tool loop is rejected in ADR 0020, because it reads as a way round the terms.

What is still open:

- Anthropic's terms say that subscription sign-in is meant for ordinary use of Claude Code, that products should use API keys, and that running Claude Code in a product takes the Commercial Terms. Whether the Claude Code delegate may ship in the default build is **Q-33**, waiting for Anthropic's answer through the owner.

- The cargo feature `claude-code` therefore keeps the delegate out of a plain `cargo build`. The project's `run.sh` and `install.sh` turn the feature on by default, for the owner's own use; `--no-claude-code` builds without it.

- The harness's own loop does not offer a Claude subscription sign-in at all: Anthropic models there take an API key ([ADR 0019](decisions/0019-vendor-documented-sign-in.md)).

## Limits and status

- Built: Claude Code in the `oneshot` mode, and Codex. The `session` mode, one long process per tab, is a choice left to measurement (Q-34). Long sessions are Q-35.
- Ollama runs in the harness's own loop, not as a delegate. Full delegate support for it is planned, not designed.
- What a delegate can do depends on its own program and version; the harness shows its events and does not change them.

## Specs and decisions

- [ADR 0020](decisions/0020-claude-code-delegate.md): Claude Code as a delegate agent.
- [ADR 0019](decisions/0019-vendor-documented-sign-in.md): a subscription sign-in ships only where the vendor documents one.
- [ADR 0023](decisions/0023-the-log-is-the-conversation.md) and [ADR 0025](decisions/0025-one-conversation-in-every-form.md): the log is the conversation, in every form.
- IF-17, CON-57, CON-58, CON-61, CON-108, CON-133, CON-134, CON-136, CON-145, Q-33, Q-34, Q-35.
