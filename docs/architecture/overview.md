# Architecture

## Layers

```mermaid
flowchart TB
    subgraph Frontends["Frontends (IF-11)"]
        TUI["Terminal (ratatui)"]
        WEB["Browser (WebAssembly)"]
    end
    subgraph Core["Core — not pluggable"]
        LOOP["Agent loop (IF-0.1)"]
        BUS["Event bus (IF-0.2)"]
        REG["Registry (IF-0.3)"]
    end
    subgraph Plugins["Interfaces, each with default implementations"]
        PROV["Provider (IF-1)"]
        AUTH["Auth (IF-2)"]
        ROUT["Router (IF-3)"]
        TOOL["Tool (IF-4)"]
        POL["Policy (IF-5)"]
        HOOK["Hook (IF-6)"]
        MEM["Memory (IF-7)"]
        CTX["ContextStrategy (IF-8)"]
        SES["SessionStore (IF-9)"]
        VER["Verifier (IF-10)"]
        TRANS["Transport (IF-12)"]
        TRIG["Trigger (IF-13)"]
        SKILL["Skill (IF-14)"]
    end
    subgraph Platform["Platform (IF-20..25)"]
        FS["Fs"] --- PROC["Process"] --- NET["Net"] --- CLK["Clock"] --- STO["Storage"] --- TERM["Terminal"]
    end
    Frontends <--> BUS
    LOOP <--> BUS
    REG --> LOOP
    LOOP --> ROUT --> PROV
    PROV --> AUTH
    LOOP --> POL --> TOOL
    LOOP --> CTX --> MEM
    LOOP --> VER --> MEM
    LOOP --> SES
    TRANS --> TRIG
    TRIG -->|skill| SKILL --> LOOP
    TRIG -->|journal| MEM
    BUS --> HOOK
    BUS --> TRANS
    Plugins --> Platform
```

## One turn of the loop

```mermaid
flowchart TD
    A([task]) --> B[ContextStrategy builds context<br/>from task, session log, memory]
    B --> C[Router picks provider and model]
    C --> D[Provider streams the turn]
    D --> E{tool calls?}
    E -- yes --> F{Policy}
    F -- allow --> G[Tool runs within its declared capabilities]
    F -- ask --> H[/user approves or denies/]
    H --> F
    F -- deny --> I[denial goes back to the model]
    G --> J[result appended to the session log]
    I --> J
    J --> B
    E -- no --> K[Verifier runs the task's protocol]
    K -- pass, evidence recorded --> L([done])
    K -- fail --> B
```

Every box is an event on the bus (IF-0.2) and an append to the session log (INV-5), which is what makes rewind and branching (IF-9) a fold over events rather than a feature bolted on.

## Builds

- **Native:** the core, every interface crate, the default plugins and the platform's native implementation; the user's own plugin crates are added in their local build (CON-2).
- **Browser:** the same core and interface crates compiled to `wasm32-unknown-unknown` (INV-1), with the browser implementations of the platform and the plugins that need no operating system. Tools that need processes are not offered (IF-21).

## Repository layout (proposed)

```text
crates/
  core/            agent loop, event bus, registry, task states
  api/             one module per interface: traits, types, errors (semver, INV-8)
  platform-native/ platform-wasm/
  provider-anthropic/ provider-openai/
  auth-apikey/
  tools-builtin/   read, edit, shell, search, fetch
  policy-default/  context-default/  session-files/  verifier-protocol/
  memory-daemon/   memory-browser/
  frontend-tui/    frontend-web/
  transport-daemon/
  cli/             the binary: config, registry wiring, frontends
docs/
```

## The architecture map

The high-level architecture is also kept as a mind map in the project's journal, shared with every session that works on the project, its nodes pointing at the decision and specification entries. This document and that map must agree; when they differ, that is a finding to settle.
