# Open questions

| Id | Question | Needs |
|---|---|---|
| ~~**Q-1**~~ | ~~The name.~~ Settled: **harness-multitool** ([ADR 0008](../decisions/0008-name.md)). | — |
| **Q-2** | The license (MIT, Apache-2.0, or both). | the owner |
| **Q-3** | Configuration format and location (TOML in the project and in the user's config directory?). | design |
| **Q-4** | Do the chosen providers accept direct requests from a browser (CORS), and with which headers? | a check per provider |
| **Q-5** | Which ratatui backend renders to the DOM well enough for the demo? | a spike |
| **Q-6** | Out-of-process plugins in any language: when, and for which interfaces? | after M2 |
| **Q-7** | A sandbox for third-party plugins (WebAssembly components)? | after M4 |
| **Q-8** | Is "no harness runs in the browser" true? Check before saying it publicly. | research |
| **Q-9** | Trigger configuration: the match syntax (label globs, payload field conditions), how a payload maps onto a journal entry or a skill's input (templates?), and per-trigger rate limits. | design |
| **Q-10** | How a `skill` run started by a callback reports back to the caller: the callback's result URL, a reply callback, or a message to the user's channel? | design |
