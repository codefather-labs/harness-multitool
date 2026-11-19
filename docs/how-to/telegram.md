# Work from Telegram

Settings > Telegram shows the bot and who may speak.
- A message from the phone reaches the agent, which answers there.
- Asked for the test log, the agent sends it back as a file.
- A plan is approved with its button on Telegram, and the window's card is withdrawn.

<!-- how-to: telegram -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/telegram) this page plays it live (CON-187).

## In the real harness

- Set a bot token and the allowed users in Settings > Telegram (or `MULTITOOL_TELEGRAM_TOKEN` and `[telegram] allowed_users`).
- The agent answers with `telegram_send`, which takes `files` too.
- From the chat: `/open`, `/new`, `/reload`, `/restart`, and the 🛠 Service button.

## See also

[Telegram](../telegram.md), [Terminal UI: Telegram](../tui.md#telegram), CON-159, CON-161, CON-163, CON-167.
