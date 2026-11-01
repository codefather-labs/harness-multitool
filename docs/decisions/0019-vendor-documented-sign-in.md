# 0019 — A subscription sign-in ships where the vendor documents it for other apps

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-2, INV-6, INV-9, CON-11, CON-55, CON-56; replaces part of [ADR 0003](0003-authentication.md)

## Decision

ADR 0003 kept every consumer subscription out of what the project ships, because vendors may restrict their use outside their own clients. That reason does not hold where the vendor itself publishes a sign-in for apps like this one. So the project ships a subscription sign-in **only** on that ground:

- **ChatGPT plan: shipped**, as the adapter `gpt-subscription`. OpenAI [documents the flow](https://developers.openai.com/siwc/token-sharing-open-source) for open-source and locally hosted apps. The harness registers a client under its own name and presents itself as nothing else.
- **Claude subscription: not shipped.** Anthropic's [terms](https://code.claude.com/docs/en/legal-and-compliance) do not permit third-party apps to offer Claude.ai login, or to collect, store or route its tokens. Anthropic models take a Console API key or a cloud provider.

The rest of ADR 0003 stands: the `Auth` interface is public, and what a user builds for their own local build is that user's responsibility.

## Why not take a vendor's own client id

Reusing another client's id would present this harness as that client. The documented flow needs no such thing.

## Note

A vendor's terms can change. If one withdraws its flow, the adapter goes; this is not legal advice.
