# Plan before building

The agent plans before it builds. Plan mode lets it read and write only its journal and maps, so nothing in the project changes. Its plan holds:
- the decision, written to the journal;
- a mind map of the strategy;
- a flowchart of the logic.

The first plan is approved with the card in the window. The second is approved with the button on Telegram: the card in the window is withdrawn and a toast says where it was approved.

<!-- how-to: plan-mode -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/plan-mode) this page plays it live (CON-187).

## In the real harness

- Ask for a plan ("let's plan …"), or type `/plan`; `/plan off` leaves without approving.
- The agent calls `mode__plan_exit` with its plan. The card shows in the window, and buttons show on Telegram; the first answer wins (CON-163).
- `mode__plan_exit` refuses a plan with no mind map or no flowchart.

## See also

[Modes](../modes.md), [Telegram](../telegram.md), CON-161, CON-163.
