# Principles

What the project optimises for. A design that breaks one of these needs a decision record that says why.

| Id | Principle | What it rules out |
|---|---|---|
| **P-1** | **Plugin-first.** Almost every component is an interface with a default implementation. The core is the agent loop and the event bus, nothing else. | A feature wired into the core that a user cannot replace. |
| **P-2** | **Your own build is a first-class build.** A plugin is a crate implementing an interface; the user adds it to a local build and selects it by name in the configuration. | Plugins that only the project can ship. |
| **P-3** | **Done means proven.** A task is complete when its verification protocol has run and the evidence is recorded — not when the model says so. | "Done" as a claim. |
| **P-4** | **Specification first.** Behaviour, isolation boundaries, policies and invariants are written down before they are built, and checked after. | Behaviour that exists only in code. |
| **P-5** | **Memory over summary.** What a session learned is kept as structured memory (journal entries, mind maps) and context is rebuilt from it. | Lossy summaries as the only record of a long session. |
| **P-6** | **Portable core.** The core and every default plugin that does not need an operating system compile to native targets and to WebAssembly. | Platform calls in the core. |
| **P-7** | **Local-first.** No hosted service of the project sits in the loop. A user's keys go to the provider they chose and nowhere else. | Telemetry or proxies that see prompts, code or keys. |
| **P-8** | **Open source, honest about limits.** The project says what it does not do (e.g. no shipped subscription login) and why. | Grey-zone features shipped by the project. |
| **P-9** | **Many agents are the normal case.** Sessions are addressable and can work together, across machines and models. | A design that assumes one agent in one terminal. |
| **P-10** | **A mode is a framework, not a prompt.** A harness mode — plan, step-debug, reliability monitoring — comes with its tools, hooks, contracts, specifications and memory: the plan is journal entries and maps, step-debug has a router contract and live charts, monitoring has a callback contract, conditions and response policies. Documentation of the tools is kept current, spec-first (P-4). | A mode that is only an instruction to the model. |
