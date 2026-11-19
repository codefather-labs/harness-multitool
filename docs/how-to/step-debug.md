# Debug a pipeline step by step

The release pipeline hangs, and the agent debugs it in step-debug mode:
- it draws the pipeline as a flowchart;
- callbacks move the flowchart one step at a time, each step waiting for its approval;
- publish is found waiting on a token the runner never had;
- the bug and its fix go to the journal, and the run is saved.

<!-- how-to: step-debug -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/step-debug) this page plays it live (CON-187).

## In the real harness

- Ask to debug ("let's debug …"), or type `/stepdebug`. The agent turns `mode__debug` on.
- The pipeline is instrumented with callbacks to the session's URL; `flowstate_update` moves the flowchart (ADR 0012).
- Ctrl+G opens or closes the newest flowchart instance. Each finished cycle is saved to `.harness/debug/runs.sqlite`.

## See also

[Modes](../modes.md), [Terminal UI: Step-debug mode](../tui.md#step-debug-mode), ADR 0012.
