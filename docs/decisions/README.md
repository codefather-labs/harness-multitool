# Decisions

One file per decision: what was decided, by whom, why, and what was rejected. A decision is replaced only by a new one that says so. Status: `accepted`, `proposed`, `replaced by NNNN`.

| ADR | Decision | Status |
|---|---|---|
| [0001](0001-plugin-first.md) | Plugin-first: almost every component is an interface | accepted |
| [0002](0002-plugins-are-crates.md) | A plugin is a crate built into the local build, selected in config | accepted |
| [0003](0003-authentication.md) | API-key auth shipped; no subscription login shipped | accepted |
| [0004](0004-platform-layer-and-browser-build.md) | Platform behind interfaces; a WebAssembly build of the CLI on the site | accepted |
| [0005](0005-own-repository.md) | A new, self-contained repository; the memory backend is carried over | accepted |
| [0006](0006-providers.md) | Anthropic API, OpenAI-compatible and local models from v1 | accepted |
| [0007](0007-features-after-mvp.md) | After the MVP: proof, memory-based context, rewind and branches, agent mesh | accepted |
