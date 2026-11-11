# 0027 — Windows talk to the daemon over a local socket

- **Status:** accepted · **Decided by:** the project owner, 2026-11-11 (the agent's design of the details)
- **Specs:** CON-130, CON-131; builds on [ADR 0024](0024-conversations-live-in-the-daemon.md)

## Why

Windows and the daemon got in each other's way for two reasons.

1. **HTTP and polling.** A window sent its operations as HTTP requests and fetched its events with long polls: one held request per tab and one per window, and a new request after every event. While the daemon served from a fixed pool of threads, held polls queued every other operation for seconds.
2. **The contract was the binary.** A window worked with a daemon only if both came from the same file: same size, same modification time. After any rebuild, a window that found a daemon busy with another window's run went on without it, Local, without the daemon's modes, agent tools or reload, and a reload waited until every other window's run had ended.

## Decision

- **One local socket.** The daemon listens on an AF_UNIX socket, `<data dir>/daemon/daemon.sock`, on every OS, Windows 10 1803 and later included. `address.json` names it. Only its user can open it, so no token is needed. Callbacks from outside stay HTTP on the daemon's port: their URLs are given to CI and scripts.
- **One connection per client, kept open.** Messages are newline-delimited JSON. A client opens with `hello` (protocol number, build, role: window, mcp or cli) and is answered `welcome` (protocol, pid). Requests carry an id and are answered with it. The daemon pushes a window's events on its connection, so nothing is polled. Every connection has its own reader, writer and queue, so a slow or stuck client holds up no other.
- **Compatibility is the protocol number.** A window works with any daemon that speaks its protocol, whatever its build. A daemon of another protocol is replaced when it is idle. Otherwise the window says so and keeps trying. A reload swaps the window at its own safe point and never waits on the daemon.
- **Clients never give up.** A client that loses its connection tries again every 2 seconds, forever, reading `address.json` again each time, so a replaced daemon is found. A window shows a toast every time its connection is lost, and every time one is made, the first one included. A request already written when the connection dropped is answered with an error and not resent, so nothing is applied twice. One not yet written waits for the next connection. When a connection comes back, every tab reopens its conversation.

The transport is the `socket2` crate on every OS, so there is one code path.

## Changeover

The old HTTP window API goes in one step: windows still open on older builds have to be reopened once. The owner warned the agent of the other project that shares this daemon first.
