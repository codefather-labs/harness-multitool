# Rely on the daemon

The window connects to the daemon, which hosts its conversations, and a task starts. The daemon then goes away:
- the window says so and retries every 2 seconds;
- the conversation goes on in the daemon meanwhile;
- back, the window shows what the agent did while it was away.

<!-- how-to: daemon -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/daemon) this page plays it live (CON-187).

## In the real harness

- The window starts the daemon when needed (`multitool daemon`); `install.sh` runs it as a user service.
- A toast shows every loss and every connection. While the daemon is down, a second toast counts the attempts, and what you send is refused at once with a toast (CON-131, CON-132).
- Settings > Behaviour chooses what a window's agents do when it leaves (CON-181).

## See also

[Daemon](../daemon.md), [Terminal UI: The daemon](../tui.md#the-daemon), ADR 0024, ADR 0027.
