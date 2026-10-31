# 0006 — Providers from v1

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** CON-10, IF-1, IF-3

## Decision

v1 supports the Anthropic API, OpenAI-compatible APIs, and local models served over an OpenAI-compatible endpoint. `Router` may use different providers and models for different steps of one task.
