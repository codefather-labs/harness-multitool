# 0026 — Profiles: every setting, and each window's sign-in

- **Status:** proposed · **Decided by:** the project owner, 2026-11-10
- **Specs:** CON-120, CON-121, CON-122, CON-123, CON-124, CON-125; keeps CON-56, CON-87, INV-6

## Decision

Every setting lives in a profile, a whole configuration of its own, and each window of the harness runs on one. So one window can work under one Claude account, another under a second Claude account, a third on a ChatGPT plan, at the same time and without conflict. A window chooses its profile with `multitool --profile <name>` or from the settings' Profiles tab.

## Where profiles live

All of a profile is in one folder, `~/.harness/profiles/<name>/`: `profile.toml` with its settings, `claude/` with its Claude Code login, `chatgpt/` with its ChatGPT credential. The folder belongs to the user's machine, not to a project: every project opened on a profile uses the same sign-in. A fresh install has `default`, which runs on the user's own `~/.claude` login. Telegram and voice are in `~/.harness/shared.toml` (the owner, 2026-11-10).

## Why this way

Claude Code supports several accounts by design: each account has its own configuration directory, named by `CLAUDE_CONFIG_DIR`, holding its own login, settings and history ([Authentication](https://code.claude.com/docs/en/authentication), "Log in with multiple accounts"). A profile names that directory, and signing in runs the unmodified `claude auth login` with it. The token stays in Claude Code's own store and the harness never sees it.

The owner first asked whether `claude` can take a token kept in the harness's config. It can (`claude setup-token` and `CLAUDE_CODE_OAUTH_TOKEN`), but Anthropic's terms say developers may not collect, store or intermediate Claude.ai credentials ([Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance)), and CON-56 forbids it here. The directory route reaches the same goal inside the terms.

The harness already carries a window's environment to the daemon in memory, and the delegate runs with it; this is where a profile's directory and keys travel.

## How a user signs in

1. Settings, Profiles, Add, a name such as `work`. The profile is clean: defaults, and its own empty Claude Code directory, `~/.harness/profiles/work/claude/`.
2. Choose the Claude Code adapter in it, and press Sign in (or run `multitool login claude-code --profile work`).
3. The harness hands the terminal to the unmodified `claude auth login`, run with that directory. The browser opens Anthropic's page; the user signs in with the account this profile is for, and Claude Code saves the login in that directory.
4. The list shows the profile as signed in. `multitool --profile work`, or Use in the tab, starts work on it.

No global sign-in is needed first, and no token is copied anywhere: each profile's login is made once, into its own directory, and Claude Code refreshes it itself. A second profile is the same steps; in the browser the user picks the other account. The user's own `~/.claude` login is untouched and stays what `default` runs on until its directory is set.

## What is not per window

One daemon serves the machine, so what it holds once is shared: journals, maps, callbacks, the orchestra and Telegram. Telegram's settings, and voice, which only sets how the daemon transcribes Telegram voice notes, are cross-profile: their own file, `shared.toml`, belonging to no profile (the owner, 2026-11-10).

## Rejected

- **A token in the harness's config.** Against CON-56 and the vendor's terms, and a risk to the account.
- **Profiles layered over one another.** Saves repeating a few fields, but then what a window runs with is not in one place.
- **A new profile copied from another.** The owner wants a new profile entirely clean: built-in defaults, no keys, no sign-in, its own empty Claude Code directory.
- **One bot per profile.** Would change CON-87 and run several bots in one daemon, for no need the owner named.
- **A daemon per profile.** Journals, Telegram and the orchestra would split by account, which is the opposite of what is wanted.
