# 0003 — Authentication: API keys shipped, no subscription login shipped

- **Status:** accepted, part replaced by [ADR 0019](0019-vendor-documented-sign-in.md) · **Decided by:** the project owner, 2026-10-31
- **Specs:** P-8, INV-6, INV-9, CON-11, IF-2

## Decision

The project ships `Auth` implementations for API keys. It does not ship, and will not accept, an implementation that signs in with a model vendor's consumer subscription. The `Auth` interface is public; a user may write such an implementation for their own local build. Responsibility for that — including compliance with the vendor's terms — lies with the user who builds it, and the README says so.

## Why

Vendors may restrict the use of consumer subscriptions outside their own clients; distributing a client that does it would put the project in that position. A public interface keeps the harness usable with whatever credentials a user is entitled to use, without the project distributing the grey part.

## Note

Whether a given vendor's terms allow a user's self-built client is not something this project assesses; it is not legal advice.
