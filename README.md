# harness-multitool

**A plugin-first, open-source agent harness in Rust.** An agent that writes code, runs tools and works with other agents — where almost every part is an interface you can replace with your own implementation, and where "done" means proven.

Status: **specification**. No code yet. This repository starts with its spec, built and checked against from the first line of code.

## What it is meant to be

- **Plugin-first.** The core is an agent loop and an event bus. Model providers, authentication, tools, permissions, context management, memory, sessions, model routing, verification, user interfaces, agent transport and hooks are interfaces. Each one has a default implementation; anyone can write another, build it into their local build and select it in the configuration.
- **Your own tools.** Tools are an interface like everything else; the built-in ones (read, edit, shell, search) are plugins of that same interface.
- **Done means proven.** The harness does not let an agent report a task done until the task's verification protocol has run and its evidence is recorded. Specifications are acceptance checks, not prose.
- **Memory that survives.** A project journal (decisions, findings, specifications, constraints, linked into a graph) and mind maps are first-class; context is rebuilt from them instead of being summarised away.
- **Many agents, many machines.** Sessions are addressable, message each other, and can be spread across machines and models.
- **Runs in the browser.** The core compiles to WebAssembly; the project site will host the CLI itself as a live demo.

## Documents

| | |
|---|---|
| [Principles](docs/spec/principles.md) | what the project optimises for, and what it refuses |
| [Invariants](docs/spec/invariants.md) | what must hold at all times, and how each is checked |
| [Constraints](docs/spec/constraints.md) | rules, limits and constants |
| [Interfaces](docs/spec/interfaces.md) | every pluggable component: its contract and its boundaries |
| [Architecture](docs/architecture/overview.md) | how the pieces fit, with a map |
| [Decisions](docs/decisions/) | why the project is the way it is (ADRs) |
| [Roadmap](docs/spec/roadmap.md) | the order of work |
| [Open questions](docs/spec/open-questions.md) | what is not decided yet |
| [References](docs/references.md) | projects we learn from |

Every rule in these documents has an id (`P-…`, `INV-…`, `CON-…`, `IF-…`). Code, tests and decisions cite them.

## On authentication and subscriptions

The project ships authentication for API keys. It does **not** ship, and will not accept, an implementation that signs in with a consumer subscription of a model vendor. The `Auth` interface is public: whoever writes and builds such an implementation for their own use does so under their own responsibility and the vendor's terms (see [ADR 0003](docs/decisions/0003-authentication.md)).
