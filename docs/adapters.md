# Adapters and providers: Claude Code, Codex, Ollama and APIs

The harness runs an agent in one of two ways: its own agent loop on a model provider, or an unmodified agent program (Claude Code, Codex CLI) as a delegate. One conversation can move between them at any moment and stays the same conversation.

## What it is

An adapter is what runs a session's turns (`AgentBackend`, IF-17). There are two kinds:

- **The harness's own loop on a provider.** The harness sends the history to a model API, runs the tools itself and holds the modes. Providers are the Anthropic API, any OpenAI-compatible API, local models on such an endpoint, and a ChatGPT plan.
- **A delegate.** The harness runs an agent program as it ships, signed in by the user through the program's own flow. The program keeps its own system prompt and tools; the harness adds its tools and shows what the program does.

Every adapter writes the same events to the same log, so the transcript, a resume and the status line work the same way on each ([ADR 0020](decisions/0020-claude-code-delegate.md)).

## How it works

### The harness's own loop

| Provider row | What it reaches | How it signs in |
|---|---|---|
| `anthropic` (the default, model `claude-opus-5-5`) | the Anthropic API | `ANTHROPIC_API_KEY`, or a key saved on the Provider page |
| kind `openai` | OpenAI, Azure, Groq, OpenRouter and other OpenAI-compatible APIs | `<PROVIDER>_API_KEY`, or `[session.auth.<name>]` |
| `ollama` (CON-190) | a local Ollama on its own API, `http://127.0.0.1:11434` | none needed |
| `local` | vLLM, LM Studio and other local OpenAI-compatible servers, at `http://127.0.0.1:11434/v1` by default; a `local` row pointing at Ollama works as before | none needed |
| `gpt-subscription` | your ChatGPT plan | OpenAI's own sign-in page |

The ChatGPT plan is shipped because OpenAI documents a sign-in flow for open-source and locally hosted apps. The harness registers its own client under its own name ([ADR 0019](decisions/0019-vendor-documented-sign-in.md)).

A Claude subscription is not offered on this loop. Anthropic's terms do not permit third-party apps to offer Claude.ai login or to collect, store or route its tokens, so Anthropic models here take a Console API key (CON-56).

Ollama and other local models are a provider of this loop: a row of kind `openai` with a `base_url`, as in the example under [Provider, model and keys](tui.md#provider-model-and-keys). The provider reads the reasoning that Ollama streams apart from the answer (`crates/provider-openai/src/lib.rs`).

#### Ollama on its own API (CON-190)

The `ollama` row reaches Ollama on its native `/api/chat`, not on its OpenAI-compatible `/v1`. The reason: `/v1` cannot set the context size. Ollama 0.33.3 loads a model there with 32 768 tokens, while the harness counted on 128 000, so it compacted late and Ollama kept only part of the prompt (journal#1532).

- **The context.** Every request sends `options.num_ctx` equal to the row's `context_window`: 65 536 by default, never more than the model's own length from `/api/show`. The window the status line shows is the one Ollama loads.
- **Capabilities.** Read from `/api/show` once per model. Tools go only to a model that has `tools`; a model without them talks only, which the model list in Settings says (`no tools: talks only`). A model with `thinking` gets `think: true`, and its reasoning is drawn apart from the answer, folded as 'Thought for Ns'.
- **Compaction** counts the whole prompt, the system prompt and the tool schemas as well as the messages (journal#1536): on a local model's window the tools alone are about 4.5k tokens.
- **Usage.** From the last line of the stream: the prompt's count less its cached part, the cached part, and the answer's count. A local model costs nothing.
- **When it fails:**
  - nothing is listening: "Ollama is not running at <url>: start it with ollama serve, or systemctl start ollama";
  - a model is not pulled: "the model <m> is not pulled here: ollama pull <m>".
- **Models.** Settings > Provider lists the pulled ones from `/api/tags`, with size, context and capabilities. Choosing one sets its window.
- **Moving a conversation** to and from it works as for every provider of this loop: the history is built from the log for each request (CON-134).

### Delegates

How the harness runs the vendors' own CLIs, unmodified, over their JSON streams, and how that stays within their terms: [Delegates](delegates.md).

| Delegate | How it runs | How it signs in |
|---|---|---|
| Claude Code (`claude-code`) | the unmodified `claude`: `claude -p` with stream-json per message and `--resume` for the next (CON-58); always with `--chrome` (CON-133) | only by the unmodified `claude auth login`, in the profile's own folder (CON-123) |
| Codex CLI (`codex`) | the unmodified `codex`: one `codex exec --json` per message and `resume <thread>` for the next, the prompt on stdin (CON-137) | only by its own `codex login`, in the profile's own Codex folder |

The harness never reads, stores or passes on a delegate's credentials (CON-57, CON-137).

A delegate gets the harness's own tools over MCP: `multitool mcp` serves the journal, maps and flowcharts, the Telegram channel and the reload as `mcp__harness__*`, and the modes as `mcp__mode__*` ([tools.md](tools.md)).

What the user set up in Claude Code and Codex reaches a delegate on every profile: MCP servers, plugins, skills, agents, commands, hooks, rules and instructions. A sign-in is never shared between profiles (CON-145).

Each delegate process runs in a systemd user scope of its own under `multitool-agents.slice` on Linux, so its memory is counted apart from the daemon's (CON-166).

Codex runs in Codex's own sandbox, `workspace-write` unless its row sets another, and `read-only` while plan mode is on (`crates/backend-codex/src/lib.rs`).

### Profiles

A profile is a whole set of settings and sign-ins in its own folder, `~/.harness/profiles/<name>/`: `profile.toml`, and its own `claude/`, `chatgpt/` and `codex/` folders ([ADR 0026](decisions/0026-profiles.md)).

So one window can work under one Claude account, another under a second account, and a third on a ChatGPT plan, at the same time. Claude Code supports this by design: each account has its own `CLAUDE_CONFIG_DIR`, and a profile names that folder.

Profiles do not own conversations. Every conversation belongs to the daemon, and every profile sees the same ones (CON-126). A fresh install has `default`, which runs on the user's own Claude Code and Codex folders until its own are signed in (CON-120, CON-137).

### One conversation in every form

A conversation is the harness's own log, which depends on no provider (INV-17). Each provider has its own form of it ([conversations.md](spec/conversations.md)):

| Provider | Its form |
|---|---|
| Claude Code | a transcript file it keeps per folder; lines are appended on its chain |
| Codex CLI | its own session files under `$CODEX_HOME/sessions/` (`crates/backend-codex/src/conversations.rs`) |
| ChatGPT plan, OpenAI-compatible, Ollama, Anthropic API | the history in each request, built from the log every time |

Before each step the daemon pulls what a form gained into the log and pushes what the log gained to the form (INV-20). A conversation is linked, never copied or forked, and keeps its id ([ADR 0025](decisions/0025-one-conversation-in-every-form.md)).

When a conversation moves to another adapter or profile, the daemon writes it in the form the target reads, and the target goes on with the whole history. When it moves back, the daemon compares a hash of the form with a hash of the log, and writes only what the form lacks, or nothing (CON-134, CON-138).

What a provider receives is always valid for it: two user messages in a row become one, a tool call with no result gets "no result in the record", and a block another model signed goes only to the model that made it (CON-73).

#### Tool names across adapters (CON-191)

Each program names its tools its own way: Claude Code's `Read {file_path}`, `Bash`, `mcp__harness__journal_add` are the own loop's `read {path}`, `shell`, `journal_add`. A model imitates the history it is given, so a moved conversation is given in the names of the program it is now in. One table maps a past call and its arguments:

| Claude Code | the own loop |
|---|---|
| `Read {file_path}` | `read {path}` |
| `Edit {file_path, old_string, new_string}` | `edit {path, old_text, new_text}` |
| `Write {file_path, content}` | `edit {path, old_text: "", new_text}` (one way) |
| `Bash {command}` | `shell {command}` |
| `Grep {pattern, path}` | `search {text, path}` |
| `WebFetch {url}` | `fetch {url}` |
| `mcp__harness__<name>` | `<name>` |
| `mcp__mode__<name>` | `mode__<name>` |

On the own loop the mapping is made on each request, and the log keeps what was said. In Claude Code's form the calls are written under Claude Code's names. A call to a tool the session does not have is answered with the one meant: "no tool named Read; this session's tool for it is read {path}". Codex's names (its calls run through a shell) are to come.

You can also leave the harness: `claude --resume <id>` opens the same conversation in Claude Code, with every turn made in the harness already in its file. While Claude Code has it open, the harness shows it but refuses to send (CON-78).

## Using it

- **Settings > Provider** chooses the provider and the model. The button under the list signs in: Login for `gpt-subscription`, a masked key for an API provider, Sign in for Claude Code and Codex. The model row lists what the provider's API lists, with the price per 1M tokens where known.
- **Ctrl+S** saves the choice. Every open conversation of the window moves onto it at once, with its history.
- **Settings > Profiles** lists, adds, edits, deletes and uses profiles. Use (or Enter) moves the window and its tabs onto that profile (CON-125, CON-127).

From the command line:

```bash
multitool login chatgpt|claude-code|codex     # sign the current profile in; logout signs it out
multitool profile list
multitool profile add work                    # a clean profile: defaults, no key, no sign-in
multitool --profile work                      # this run only
multitool import claude-code [id]             # list or link this folder's Claude Code conversations
```

In the configuration, a provider row looks like this (`config.toml`, see `multitool setup`):

```toml
[session]
provider = "local"

[session.providers.local]
kind = "openai"
base_url = "http://127.0.0.1:11434/v1"
context_window = 32000

[session.auth.local]
none = true
```

While the daemon writes a conversation's form for another adapter or profile, that tab shows a toast with a progress bar: what is being done, messages (or MB) done out of the total, and the share. Only that tab is held (CON-136, CON-144).

## Limits and status

- **Claude Code as a delegate is in the default build** (CON-57, Q-33 settled 2026-11-19). `--no-default-features`, or `--no-claude-code` in `run.sh` and `install.sh`, builds without it.

- **Ollama is a provider of the harness's own loop, not a delegate.** Ollama has no agent program of its own. The owner chose the own loop on Ollama's native API for now (CON-190, journal#1533). Codex or Claude Code run against a local Ollama server stays open (journal#1222).

- The adapter, model and context window cannot be changed while a run of the window is on: the controls are disabled, and the daemon refuses (CON-79).

- Two programs cannot write one conversation at once. A resume started in Claude Code during a harness run makes a branch; the file keeps both ([ADR 0025](decisions/0025-one-conversation-in-every-form.md)).

- Claude Code's and Codex's file formats are their own and undocumented, and can change with a version. A form that cannot be read or written is an error on screen, never a silent loss.

- A delegate runs as `claude -p` or `codex exec`, so a program's interactive-only tools (asking with options, its own plan mode) are the harness's own there, or absent.

- Background tasks shown above the composer come only from the Claude Code delegate today; the other adapters are a deferred todo (journal#402).

## Specs and decisions

- [ADR 0006](decisions/0006-providers.md) providers from v1; [ADR 0019](decisions/0019-vendor-documented-sign-in.md) subscription sign-in only where the vendor documents it.

- [ADR 0020](decisions/0020-claude-code-delegate.md) Claude Code as a delegate; [ADR 0022](decisions/0022-claude-code-conversations.md) moving a Claude Code conversation onto the harness.

- [ADR 0024](decisions/0024-conversations-live-in-the-daemon.md) conversations live in the daemon; [ADR 0025](decisions/0025-one-conversation-in-every-form.md) one conversation in every form; [ADR 0026](decisions/0026-profiles.md) profiles.

- [Conversations](spec/conversations.md): INV-15 to INV-21.

- [Constraints](spec/constraints.md): CON-56, CON-57, CON-58, CON-73, CON-78, CON-79, CON-120, CON-123, CON-126, CON-133, CON-134, CON-136, CON-137, CON-138, CON-144, CON-145, CON-166.

- [Terminal UI](tui.md): [Profiles](tui.md#profiles), [Provider, model and keys](tui.md#provider-model-and-keys), [Claude Code and Codex as delegates](tui.md#claude-code-and-codex-as-delegates).
