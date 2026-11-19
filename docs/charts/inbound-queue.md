# Shared pending messages (CON-116)

How typed messages and messages from outside wait in one queue, and when the session consumes them.

Plan for journal#661, work journal#665, decision journal#664; maps 29 (strategy) and 30 (delivery flow).

Telegram, callbacks, a2a and typed messages share the existing CON-83 FIFO. Entries carry their kind: user text or labelled inbound. Pending outside messages show their source above the composer. They remain external data in the model history and log, and cannot approve a plan. Up retrieves only the last pending user entry.

At the next engine boundary, after tool results, entries are consumed once in arrival order. A labelled entry emits Inbound, not UserMessage. If idle, or if the run finishes before consumption, the session wakes and starts the next run from the remaining first entry. Existing ownership locks, stop/rebind behavior and empty-wakeup suppression remain.

Implementation touches API/shared queue representation, core consumption, session routing, daemon queue transport, and TUI pending rendering/input.

Verification first reproduces the delay, then checks delegate and native-provider active delivery, visible source-labelled pending rows, mixed FIFO, once/no extra run, idle fallback, Up ownership and approval isolation. Regression PTYs include composer, a2a, Telegram topics, orchestra and queue/stop/rebind scenarios. The running user's harness needs reload and live confirmation after build; tests alone do not establish that reload happened.
