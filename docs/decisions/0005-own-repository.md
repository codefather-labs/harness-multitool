# 0005 — A new, self-contained repository

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** CON-20, CON-40

## Decision

The harness is its own project in its own repository, an evolution of an earlier harness grown into something separate. The earlier work's backend — a local daemon with the project journal, mind maps, messaging channels, an agent registry and pools — is carried over into this repository as the default `Memory` and `Transport` implementations. The repository does not refer to the earlier project.

## Why

A product with its own name, release cycle and audience; the backend is proven and worth keeping, its identity is not.
