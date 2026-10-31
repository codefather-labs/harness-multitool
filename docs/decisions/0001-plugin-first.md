# 0001 — Plugin-first

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** P-1, P-2, INV-2, INV-7, IF-0.3

## Decision

Almost every component of the harness is an interface with a default implementation: providers, authentication, routing, tools, policy, hooks, memory, context strategy, session store, verifier, frontends, transport, and the platform itself. The core is the agent loop, the event bus and the registry. Being open source, anyone can implement any part.

## Why

Existing harnesses are products with extension points; this one is a set of interfaces with a product on top. That is what lets a user replace exactly the part they disagree with — the provider, the policy, the way context is built — without forking the rest, and it is the project's main difference.

## Rejected

- A fixed product with a plugin API for tools only: covers the common case, but every deeper change means a fork.
