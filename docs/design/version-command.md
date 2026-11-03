# TUI /version

Specification: CON-102, journal#515 (replaces journal#504, deprecated). Work item: journal#505. Mind map #6; flowchart #7.

`/version` appends an informational `Item::Tips` to the current transcript, not a toast, with label `VERSION` and text `harness-multitool <version>`. The version comes from `env!("CARGO_PKG_VERSION")`; frontend-tui inherits the workspace package version. No hard-coded version, git hash, provider request or daemon command is needed.

The local handler and `/help` entry are in `crates/frontend-tui/src/debug.rs`; usage is documented in `docs/tui.md`.

Verification journal#509: frontend tests 104 passed, 0 failed, 1 ignored. Tests submit `/version` in preview and live workspaces and check the label, package version, unchanged outbox, help discovery, and normal message regressions. The built CLI's preview in a PTY displayed VERSION and harness-multitool 0.1.0-dev. Interactive verification was preview only, not a live provider.

