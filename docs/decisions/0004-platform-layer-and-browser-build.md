# 0004 — Platform behind interfaces; the CLI in the browser

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** P-6, INV-1, INV-10, CON-5, CON-30, CON-31, IF-20..IF-25

## Decision

The project site hosts a WebAssembly build of the CLI as a live demo. Therefore the core never touches the operating system: files, processes, network, clock, storage and the terminal are Platform interfaces with native and browser implementations.

## Why

A harness that runs in the page — its loop, its tools over a virtual file system, its proof gate, its live mind map — is a demo no screenshot can match. The owner's view is that no comparable harness offers this; that claim is to be checked before it is used publicly.

## Consequences

- No shell, processes or daemon in the browser; tools that need them are not offered there.
- The user's key is entered and kept in the browser and sent only to the chosen provider.
- To check before building: whether each provider accepts direct calls from a browser (CORS), and the readiness of a ratatui-to-DOM backend.
