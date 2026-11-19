# Sign in to each adapter

Settings > Provider shows what the window's profile is signed in to, row by row.
- Claude Code is signed in by its own `claude auth login`, and Codex by its own `codex login`, each in the profile's own folder.
- A ChatGPT plan signs in on OpenAI's own page.
- An API key is typed into the page and shown masked.

In the player every sign-in is mocked: no account, page or key is real.

<!-- how-to: sign-in -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/sign-in) this page plays it live (CON-187).

## In the real harness

- In the settings, Provider is the first page. The button under the list signs the chosen row in: Sign in for Claude Code and Codex, Login for `gpt-subscription`, a masked key for an API provider.
- From the command line: `multitool login claude-code`, `multitool login codex`, `multitool login chatgpt`, and `multitool logout …` for the current profile.
- The harness never reads, stores or passes on a delegate's credentials (CON-57, CON-123, CON-137).

## See also

[Adapters and providers](../adapters.md), [Delegates](../delegates.md), [Profiles](../tui.md#profiles).
