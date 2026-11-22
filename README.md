<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/design/readme/hero-dark.png">
    <img alt="harness-multitool's welcome screen: the logo, and what the harness does, point by point." src="docs/design/readme/hero-light.png" width="860">
  </picture>
</p>

<p align="center">
  <a href="https://harness-multitool.dev">harness-multitool.dev</a> ·
  <a href="#install">Install</a> ·
  <a href="#what-it-does-today">What it does</a> ·
  <a href="#documents">Documents</a> ·
  <a href="docs/tui.md">Terminal UI</a> ·
  <a href="docs/spec/roadmap.md">Roadmap</a>
</p>

**The next generation of AI harness: plugin-first, in Rust.** One harness for Claude Code, OpenAI and Ollama. Its agents write code, run tools and keep the project's knowledge in a journal, mind maps and flowcharts. Almost every part of it is an interface you can replace. Try it in your browser at [harness-multitool.dev](https://harness-multitool.dev).

## Install

Linux and macOS:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://harness-multitool.dev/install.sh | sh
```

Windows, in PowerShell:

```powershell
irm https://harness-multitool.dev/install.ps1 | iex
```

Each line downloads the prebuilt `multitool` for your machine from the [latest Release](https://github.com/codefather-labs/harness-multitool/releases/latest), checks its SHA-256, and installs it: on your PATH, with its daemon as a user service (a task at logon on Windows) and Claude Code's rules. The builds are Linux x86_64 and aarch64 (glibc 2.31 or newer), macOS on Apple silicon and Windows x86_64. No Rust is needed. Options go after `sh -s --`, for example `| sh -s -- --no-service`; on Windows, `& ([scriptblock]::Create((irm https://harness-multitool.dev/install.ps1))) -NoService`. On another system the line says how to build from source, and installs nothing (CON-186, CON-195).

`install.sh` and `install.ps1` of this repository do the same when run on their own: they install the latest release, or the one given with `--version` / `-Version`. They build only when asked: `--from-source` / `-FromSource`, or `--local` / `-Local` in a checkout.

Every commit of `main` that passes CI is released by GitHub Actions (CON-194). The version is `<Cargo version>+<commit>`, for example `0.1.0-dev+9e5f4de`. The commit is tagged `v<version>`, and its GitHub Release holds the builds for Linux x86_64 and aarch64, macOS on Apple silicon and Windows x86_64, each with its checksum. Every install line downloads from it. The newest Release is marked Latest, and each version keeps its own tag. A version is published once and never changes: a fix is a new commit and a new version (CON-193).

## Quick start from source

```bash
git clone https://github.com/codefather-labs/harness-multitool
cd harness-multitool
./run.sh            # builds it and opens the terminal UI (Windows: run.ps1)
```

Open the settings with `Ctrl+,` and choose an adapter. The options are: Claude Code or Codex, each signed in by its own program; a key for an API; a local model on Ollama; or your ChatGPT plan through `gpt-subscription`, which signs in on OpenAI's own page.

## What it does today

- **One harness, many adapters.** The unmodified Claude Code and Codex CLIs run as delegates over their JSON streams. The harness's own loop runs on Anthropic and OpenAI-compatible APIs, Ollama and other local servers, or a ChatGPT plan. A conversation moves between them and stays one conversation ([Adapters](docs/adapters.md), [Delegates](docs/delegates.md)).
- **The agent loop in a terminal UI:** streaming answers with Markdown and tables, tool calls with permission prompts, tabs and sessions that resume. Profiles keep their own sign-ins, and the status line shows what the agent is doing ([Terminal UI](docs/tui.md)).
- **Knowledge graph.** A project journal holds the decisions, findings and changes, with mind maps and live flowcharts linked to it. The agent's context is built from it ([Journal](docs/journal.md), [Maps](docs/maps.md)).
- **Done means proven.** A task closes only on a verification entry with its evidence ([Agent workflow](docs/agent-workflow.md)).
- **Docs that stay current.** Specs and invariants have ids, and `multitool docs check` keeps the documentation and the journal in agreement.
- **Modes with their own tools** ([Modes](docs/modes.md), [Orchestration](docs/orchestration.md)):
  - plan mode: nothing changes until you approve, in the window or on Telegram;
  - orchestration: one agent starts others and closes their tasks on proof;
  - step-debug of running pipelines.
- **Agents that talk to each other.** Every conversation on the machine can reach another by its nick ([Agents](docs/agents.md)).
- **The daemon.** It hosts every conversation, keeps agents working when a window closes, and is the single entry point of callbacks ([Daemon](docs/daemon.md)).
- **Callbacks.** Any app wakes an agent through its callback URL, and a project's own handlers are Rust programs ([Callbacks](docs/callbacks.md)).
- **Telegram.** Talk to your agents, approve plans and get files back from your phone ([Telegram](docs/telegram.md)).
- **Conversations as files.** A conversation dumps to a .zip with its journal and maps, and loads here or on another machine ([Terminal UI](docs/tui.md#chats)).
- **In the browser.** The real terminal UI, compiled to WebAssembly, runs on [harness-multitool.dev](https://harness-multitool.dev).

## What it is meant to be

- **Plugin-first.** The core is an agent loop and an event bus. Model providers, authentication, tools, permissions, context, memory, sessions and routing are interfaces, and the built-in ones are plugins of those same interfaces.
- **Modes are frameworks, not prompts.** A mode gives the agent its rules and tools, and the harness holds to them.
- **Many agents, many machines.** Agents already message each other on one machine. Daemons on other machines, as orchestrators and sync nodes, are planned ([Roadmap](docs/spec/roadmap.md)).
- **AI-first reliability monitoring, 24/7.** A concept, not built.

## Documents

| | |
|---|---|
| [Principles](docs/spec/principles.md) | what the project optimises for, and what it refuses |
| [Invariants](docs/spec/invariants.md) | what must hold at all times, and how each is checked |
| [Constraints](docs/spec/constraints.md) | rules, limits and constants |
| [Interfaces](docs/spec/interfaces.md) | every pluggable component: its contract and its boundaries |
| [Architecture](docs/architecture/overview.md) | how the pieces fit, with a map |
| [Terminal UI](docs/tui.md) | running it, keys, debug commands, screens as images |
| [Journal](docs/journal.md), [Maps](docs/maps.md) | the project's knowledge graph: the journal, mind maps and flowcharts |
| [Modes](docs/modes.md), [Orchestration](docs/orchestration.md) | plan, debug and orchestrate: what each mode gives an agent |
| [Adapters](docs/adapters.md), [Delegates](docs/delegates.md) | Claude Code, Codex, Ollama and APIs; how the vendors' CLIs run inside the harness, within their terms |
| [Agents](docs/agents.md), [Daemon](docs/daemon.md), [Telegram](docs/telegram.md) | agents that message each other, the daemon that hosts them, and Telegram |
| [Agent workflow](docs/agent-workflow.md) | what an agent must do in each situation; ships with the harness, and agents are given its path |
| [Tools and modes](docs/tools.md) | what an agent can call, and how the harness's modes differ from utilities |
| [Callbacks](docs/callbacks.md) | how outside apps reach an agent, and how to write a project's own callback handler in Rust: the contract |
| [Design](docs/design/README.md) | the logo, icons, themes and the mockups the UI is built against |
| [Decisions](docs/decisions/) | why the project is the way it is (ADRs) |
| [Roadmap](docs/spec/roadmap.md) | the order of work |
| [Open questions](docs/spec/open-questions.md) | what is not decided yet |
| [References](docs/references.md) | projects we learn from |

Every rule in these documents has an id (`P-…`, `INV-…`, `CON-…`, `IF-…`). Code, tests and decisions cite them.

## On authentication and subscriptions

The harness never collects or routes anyone's credentials.

- **Claude Code and Codex** run as their vendors ship them. Each user signs in to their own copy through the vendor's own flow, so a Claude subscription works inside Claude Code ([Delegates](docs/delegates.md), [ADR 0020](docs/decisions/0020-claude-code-delegate.md)).
- **The harness's own loop** takes API keys. It offers a subscription sign-in only where the vendor documents one for apps like this ([ADR 0019](docs/decisions/0019-vendor-documented-sign-in.md)):
  - a ChatGPT plan signs in through OpenAI's flow for open-source apps;
  - Anthropic models take an API key, as Anthropic's terms do not permit a Claude.ai login in other apps.
