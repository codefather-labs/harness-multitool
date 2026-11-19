# Start for the first time

`multitool` opens on its welcome screen: what the harness is, point by point. Enter leaves it for the start page, which lists this folder's conversations and the quick actions. `n` starts a new session, whose status line shows the mode, the profile, the adapter and model, and what the conversation has spent.

<!-- how-to: first-start -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/first-start) this page plays it live (CON-187).

## In the real harness

```bash
curl --proto '=https' --tlsv1.2 -sSf https://harness-multitool.dev/install.sh | sh
multitool                 # in your project's folder
```

- On the welcome screen, Enter (or a click on OK) goes on.
- On the start page: ↑↓ choose, Tab moves between the conversations and the quick actions, Enter opens. `n` is a new session, `r` resumes one, `s` opens the settings.
- In a session, `/settings` or Ctrl+, opens the settings. `/exit` leaves.

## See also

[Terminal UI: Run](../tui.md#run), [Terminal UI: Flow](../tui.md#flow), [Keys](../tui.md#keys).
