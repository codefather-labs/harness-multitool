# The daemon

One daemon per machine hosts every conversation of the harness. Windows are its clients: an agent keeps working when its window closes, and callbacks, Telegram and other agents reach it through the daemon.

## What it is

The daemon is a background process, `multitool daemon`, that holds the conversations ([ADR 0024](decisions/0024-conversations-live-in-the-daemon.md)). Every message in and out goes through it, and it alone writes a conversation's log (INV-15, CON-75).

A session's engine runs in the daemon: its log, its tools and its calls to a provider or a delegate. A window holds a view, sends operations and hears events.

What the daemon holds once is shared by every window and every profile (CON-124):

- the conversations, whatever profile opens them (CON-126);
- the journals and maps, one per project;
- the callbacks' entry;
- the Telegram channel;
- the orchestra of agents and the games between them.

## How it works

### Windows talk to it over a local socket

Windows, the harness's MCP servers and the command line talk to the daemon over one local socket, `<data dir>/daemon/daemon.sock`, named in `address.json` beside it ([ADR 0027](decisions/0027-windows-talk-to-the-daemon-over-a-local-socket.md), CON-130). On Linux the data dir is `~/.local/share/harness-multitool/`.

- It is an AF_UNIX socket on every OS, Windows 10 1803 and later included.
- Only its user can open it, so no token is needed.
- Each client keeps one connection, with newline-delimited JSON: `hello` with the protocol number, build and role, answered by `welcome`.
- The daemon pushes each window's events on its connection; nothing is polled.
- Each connection has its own reader, writer and queue, so a slow client holds up no other.

A window works with any daemon that speaks its protocol, whatever its build. A daemon of another protocol is replaced when it is idle; while it runs other windows' conversations, the window waits and asks it every 2 seconds to stop if idle.

### HTTP for callbacks

The daemon also listens on an HTTP port on `127.0.0.1`. It serves callbacks from outside, `/health` and the dashboard, nothing a window needs.

Every conversation has its own callback URL, `http://127.0.0.1:<port>/callback/<token>`, the same across tabs, windows, reloads, daemon restarts and moves between adapters (CON-164). A callback reaches the session wherever it is open and starts a run of an idle agent. A session not open keeps it, 500 at most for 7 days. See [callbacks.md](callbacks.md).

### Agents outlive their windows

Leaving a window (/exit, or Ctrl+C twice) asks what its agents do (CON-181):

1. stop now;
2. let the command under way finish;
3. let them finish their work, then end;
4. keep them running in the daemon, where Telegram, callbacks and other agents reach them.

'Remember my choice' sets Settings > Behaviour 'On exit'. A window lost without a word (killed, its terminal or ssh dropped, silent for 30 s) does 'On an accidental loss', 3 by default.

With no window, a question an agent asks (a permission, `ask_user`, a plan approval) waits in its conversation. The next window that opens the conversation takes over its run and its question.

### When the connection drops

A window that loses the daemon tries again every 2 seconds for as long as it is open, and starts a daemon if none comes back (CON-131). It shows a toast at every loss and every connection made.

While it is down, a second toast stays up with a pulsing bar and the attempts so far, naming the reason. What you send meanwhile is refused at once with a toast, and the typed message stays in the composer.

The window never hangs on the daemon: with no connection a call fails at once, and a daemon that does not answer in 2 seconds counts as lost (CON-132). When the connection is back, every tab reopens its conversation.

### Delegate processes

Each Claude Code or Codex process the daemon starts runs in a systemd user scope of its own under `multitool-agents.slice` on Linux, counted apart from the daemon (CON-166).

No delegate outlives its daemon. On a stop the daemon ends each one, SIGTERM then SIGKILL after 3 s (CON-128). `multitool daemon reap` stops scopes left by a daemon that is gone; it runs at each daemon's start and after the service stops (CON-182).

## Using it

The window starts the daemon when it needs one. Installed with `./install.sh --local`, the daemon runs as a user service (CON-162):

| OS | Service |
|---|---|
| Linux | systemd user unit `multitool-daemon.service`, with linger enabled unless `--no-linger` |
| macOS | a LaunchAgent |
| Windows | a Scheduled Task `multitool-daemon` at logon |

Elsewhere it starts in the background with a warning. `--no-service` installs only the binary. Before restarting a daemon, the installer checks whether agents are working and asks first (CON-176).

```bash
multitool daemon                # run it in the foreground; --port picks the port on localhost
multitool daemon --status       # what it counts: conversations, Telegram, its rss_mb and peak_mb
multitool daemon restart        # stop it, its runs included, and start this build's on the same port
multitool daemon stop           # stop it and start none
multitool daemon reap           # stop delegate scopes whose daemon is gone
multitool dashboard             # print the dashboard's address with its token
systemd-cgtop multitool.slice   # each delegate's memory, apart from the daemon's
```

`multitool daemon restart` goes through the service when one is installed. Runs going on are stopped and counted, but no conversation is lost: every open window finds the new daemon and reopens its tabs (CON-128). From Telegram, `/restart` does the same after a ✅ (CON-167).

`/reload`, or the agent's `plugins_reload`, swaps the window's binary and replaces the daemon too. The daemon keeps its port, so callback URLs stay valid, though callbacks sent during the second of the swap fail ([Reload](tui.md#reload)).

### The dashboard

The daemon serves a page on its HTTP port, `/dashboard`, local only, opened with the token `multitool dashboard` prints ([ADR 0029](decisions/0029-agent-games.md)). Its tabs:

- Overview: windows, agents online, and the daemon's state;
- Games: tables of Texas hold'em between agents, live, with their history.

The token lets whoever has it see and start games, so keep it to yourself.

## Across machines

The project's principles name many agents across machines as the normal case (P-9), and the roadmap plans for it. Today the daemon serves one machine.

### What works today

- **One machine, one daemon.** The HTTP port binds `127.0.0.1` only (`crates/daemon/src/lib.rs:319`). The socket is a local AF_UNIX file readable only by its user (`crates/daemon/src/socket.rs:87`). Nothing listens on a network address.

- **Callback URLs are local.** They are `http://127.0.0.1:<port>/…` (`crates/daemon/src/lib.rs:1004`, CON-164). A program on another machine cannot reach them without a tunnel of your own; the harness has none built in.

- **The dashboard is local.** `multitool dashboard` prints a `127.0.0.1` address and says to open it in a browser on this machine (`crates/cli/src/main.rs:435`).

- **Agents talk to agents on the same machine.** `agent_list` lists "the other agents on this machine" and `agent_send` reaches them through the daemon (`crates/session/src/host_tools.rs:261`), across windows, projects and adapters.

- **Telegram reaches the daemon from anywhere.** The daemon polls Telegram outward (`getUpdates`, `crates/daemon/src/telegram.rs:426`), so no port is opened. From the phone you can write to agents, approve plans, open sessions and restart the daemon ([telegram.md](telegram.md)).

- **A window over ssh.** A window is a terminal program, so it can run on the daemon's machine over ssh. A dropped ssh is an accidental loss, and the agents go on (CON-181).

- **Moving work by hand.** `multitool chat dump` writes a conversation, its Claude Code form and the project's journal with its maps into a .zip, and `multitool chat load` brings it into a project on another machine, under new ids (CON-171, CON-175). `multitool journal export` and `journal import` carry a journal and its maps.

`crates/session/src/remote.rs` is the window's side of the local daemon, `RemoteSessions`, not a network client (`crates/session/src/remote.rs:1`).

### What is planned

- **Remote daemons are not built.** The roadmap's M3, in progress, lists them as not done ([roadmap](spec/roadmap.md)).

- **A `Transport` that reaches other machines.** IF-12 addresses sessions by name across machines, authenticated per peer, with a remote daemon over TLS as a planned implementation ([interfaces](spec/interfaces.md)).

- **An agent mesh.** M6 plans a task split across agents on different machines, each with its own model, and routing with budgets ([ADR 0007](decisions/0007-features-after-mvp.md), item 4).

- **The journal in Telegram** needs an HTTPS address the phone can reach; it is designed to go through a tunnel, and not built (journal#1234, journal#1237).

So the daemon is today a local orchestrator for the agents of one machine, reachable from outside only through Telegram. A shared orchestrator and sync node across machines is a goal on the roadmap, not a feature.

## Limits and status

- One daemon per data directory. A daemon checks `address.json` every 5 s, and stops by itself when the file names another daemon two checks in a row (CON-155).

- The daemon has no idle exit: it stays up with no window open (CON-155).

- Only a scripted run, `multitool run`, and a window that cannot have a daemon run sessions of their own. Such a window can only stop its agents when it leaves (CON-75, CON-181).

- `multitool run` through the daemon is not done yet ([roadmap](spec/roadmap.md)).

- Windows on a build of an older protocol have to be reopened once after a protocol change ([ADR 0027](decisions/0027-windows-talk-to-the-daemon-over-a-local-socket.md)).

- A restart ends the runs going on; their conversations stay (CON-128).

## Specs and decisions

- [ADR 0024](decisions/0024-conversations-live-in-the-daemon.md) conversations live in the daemon; [ADR 0027](decisions/0027-windows-talk-to-the-daemon-over-a-local-socket.md) the local socket; [ADR 0026](decisions/0026-profiles.md) what is one per machine; [ADR 0029](decisions/0029-agent-games.md) the dashboard and games.

- [Conversations](spec/conversations.md): INV-15 to INV-21, IF-19.

- [Constraints](spec/constraints.md): CON-75, CON-124, CON-126, CON-128, CON-130, CON-131, CON-132, CON-155, CON-162, CON-164, CON-166, CON-171, CON-175, CON-176, CON-181, CON-182.

- [Interfaces](spec/interfaces.md): IF-12 `Transport`, IF-19 `ConversationHost`. [Principles](spec/principles.md): P-9.

- [Terminal UI: The daemon](tui.md#the-daemon), [Leaving a window](tui.md#leaving-a-window), [Reload](tui.md#reload); [Telegram](telegram.md); [Adapters and providers](adapters.md).
