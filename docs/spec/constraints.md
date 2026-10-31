# Constraints

Rules, limits and constants. Unlike a principle, a constraint is not weighed against anything; unlike an invariant, it is a choice that could have been different and is recorded as one.

## Technology

| Id | Constraint |
|---|---|
| **CON-1** | The harness is written in Rust, stable toolchain. |
| **CON-2** | A plugin is a crate that implements an interface trait. It is compiled into the binary of a local build and selected by name in the configuration. |
| **CON-3** | No dynamically loaded Rust libraries as plugins (Rust has no stable ABI). |
| **CON-4** | Out-of-process plugins (any language, over stdio) are not part of v1. They may be added later as another way to implement the same interfaces. |
| **CON-5** | Build targets: Linux x86_64 and arm64, macOS arm64, Windows x86_64, and `wasm32-unknown-unknown` for the browser demo. |
| **CON-6** | The terminal interface is built on ratatui. |

## Providers and authentication

| Id | Constraint |
|---|---|
| **CON-10** | v1 providers: the Anthropic API, OpenAI-compatible APIs (OpenAI, OpenRouter and the like), and local models served over an OpenAI-compatible endpoint. |
| **CON-11** | Shipped `Auth` implementations: API keys (from the environment, the config, or the OS keychain on native targets). See INV-9 for what is not shipped. |

## Memory and sessions

| Id | Constraint |
|---|---|
| **CON-20** | The default `Memory` implementation is a local daemon that owns a project journal (entries with categories, tags, flags and links, hybrid lexical + semantic search) and mind maps. In the browser, the same interface is implemented over browser storage. |
| **CON-21** | Journal flags include work items (todo, question, problem, bugreport, reminder, unverified, disputed) and marks (specification, constraint, important, idea, concept, comment, verified, deprecated); an entry carries any number of them. |
| **CON-22** | One journal per project; a project is identified by its repository remote, or its directory when it has none. |

## Browser build

| Id | Constraint |
|---|---|
| **CON-30** | The browser build has no shell, no processes and no daemon: files live in an in-memory or origin-private file system, network goes through `fetch`, storage through IndexedDB. |
| **CON-31** | The user's key in the browser build is entered by the user and stored only in that browser. |

## Project

| Id | Constraint |
|---|---|
| **CON-40** | The repository is self-contained: it does not refer to, depend on or name the project it grew out of. |
| **CON-41** | Every spec item, invariant and constraint has a stable id; ids are never reused. |
| **CON-42** | A change to a principle, invariant, constraint or interface contract goes through a decision record in `docs/decisions/`. |
