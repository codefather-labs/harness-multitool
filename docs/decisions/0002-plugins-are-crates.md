# 0002 — A plugin is a crate, built in, selected in config

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** P-2, CON-2, CON-3, CON-4

## Decision

Interface crates live in this repository. A user implements an interface as a crate, adds it to their local build, and selects it by name in the configuration. The registry resolves names to the implementations linked into the binary.

## Why

Zero overhead on the hot path (streaming, tool calls), full type safety, and exactly the workflow the project wants for sensitive parts such as authentication: an implementation that exists only in the build of the person who wrote it.

## Rejected

- **Dynamically loaded Rust libraries:** Rust has no stable ABI; fragile and unsafe.
- **Out-of-process plugins as the base:** any language and isolation, but serialisation on the hot path. Kept as a possible later addition for tools, verifiers and frontends (CON-4).
- **WebAssembly components as the base:** a sandbox for third-party plugins, at the cost of complexity and limited system access. A possible later addition.
