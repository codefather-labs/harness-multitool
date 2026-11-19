# Carry a conversation anywhere: dump and load

A conversation is portable by design: it is the harness's own log, and depends on no provider. Here a conversation is built with its journal and mind map, and Settings > Chats lists this project's conversations by adapter.
- Dump packs one conversation, its journal and its maps into a .zip in `multitool_dumps/`.
- Load brings an archive back, always as a new conversation, here or on another machine.

Each shows its progress in a toast. The loaded conversation opens in a tab and goes on where it was: its history, its journal and its maps came with it, under new ids.

<!-- how-to: chats -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/chats) this page plays it live (CON-187).

## In the real harness

- Settings > Chats: Enter on a conversation shows [ Dump ]; [ Load… ] opens the system's file picker. Keys: `d` dump, `l` load.
- From the command line: `multitool chat dump [id|name]` and `multitool chat load <file.zip>`.

## See also

[Terminal UI: Chats](../tui.md#chats), CON-171, CON-172, CON-173, CON-175.
