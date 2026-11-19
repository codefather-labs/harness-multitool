# Modes: plan, step-debug

A mode is a way the harness runs an agent: while it is on, what the agent may do and what it is told change. Plan mode is for studying and planning before anything is built; debug mode is for reproducing a fix before changing anything and verifying it after.

## What it is

An agent has two sorts of thing to call ([Tools and modes](tools.md#modes), CON-96):

- **utilities** do one thing and answer, and change nothing about how the agent runs: `read`, `journal_add`, `agent_send`;
- **modes** change how the agent is run. They are entered through tools under the namespace `mode__`, and the system prompt says what a mode is.

A mode is a framework, not a prompt (P-10). It comes with its tools, hooks, rules and memory: the plan is journal entries and maps, step-debug has a router contract and live charts.

The modes:

| Mode | Entered with | Page |
|---|---|---|
| Plan | `mode__plan_enter`, or `/plan` | this page |
| Debug, with step-debug for pipelines | `mode__debug`, or `/stepdebug` | this page |
| Orchestration | `mode__orchestrate` | [Orchestration](orchestration.md) |

On a program running as an agent backend, such as Claude Code, the modes come over a second MCP server, `mode`, as `mcp__mode__plan_enter` and so on (CON-106).

## How it works

### When an agent enters a mode

The rules are in every session's system prompt ([ADR 0021](decisions/0021-agent-behaviour.md)):

- before building an app, a feature, a fix or any change that is not trivial, the agent enters plan mode, even if nobody said plan;
- answering, reading and diagnosing with no change need no mode;
- after an approved plan, a fix goes on in debug mode: reproduce it, change, verify;
- a feature is built and run, then checked in debug mode the same way.

[The agent workflow](agent-workflow.md#1-what-the-user-says) holds these as rows W-1 to W-5 and W-41.

### Plan mode

The agent studies the task and the code and writes a plan; the user approves it; only then does work begin ([ADR 0016](decisions/0016-plan-mode.md)).

In plan mode the agent can:

- read, search and fetch;
- run single read-only commands;
- write under `docs/`, since a feature is described before it is built (CON-65);
- use the journal, mind maps and flowcharts.

Anything else is refused. Entering the mode also fires the hook point `PlanEnter` for a project's own hooks.

The plan is the journal and the maps, so the next session and other agents find it ([agent workflow](agent-workflow.md#4-in-plan-mode), W-30 to W-36):

- every decision, finding and open question is a journal entry;
- each work item is a `todo` with its test protocol;
- a mind map holds the strategy, and a flowchart the logic of the planned behaviour ([Maps](maps.md#in-plan-mode)).

`mode__plan_exit` refuses a plan with no journal entry, mind map or flowchart, and a plan whose map or chart is empty (CON-117).

### Approving a plan

`mode__plan_exit` takes `plan`, the plan in short with the journal ids of its work items. There are two ways out:

- **The user has just said to go.** The agent quotes their words exactly in `user_said`. The harness checks they are in the user's last message, and the mode ends.
- **The user has not said it.** The agent leaves `user_said` out, ends its turn and waits. The user is asked in two places at once (CON-163).

The two places:

- a card in the window, like the permission card, showing the plan with approve and reject;
- the plan on the user's Telegram, in the session's bound chat or topic, with ✅ approve and ✏️ change buttons (CON-161).

The first answer decides, and the other place follows. Answered in the window, the Telegram message loses its buttons and says where it was decided. Answered on Telegram, the window's card is withdrawn and a toast says "✅ План одобрен в Telegram" or "✏️ План не одобрен в Telegram".

An approval reaches the agent with plan mode already off. A rejection asks it to find out what to change. A message from the user's Telegram counts for `user_said` like one typed in the window (CON-158).

A slave's plan is approved by its master, not by the user ([Orchestration](orchestration.md#plans-and-permissions)).

### Debug mode

`mode__debug` with `on` true switches debug mode on: the agent's instructions change to reproducing and verifying, and the window follows the run. The agent turns it off when the result is verified and the evidence is in the journal.

A small program is checked by running it and by tests; a small bug is reproduced by a failing test. A large pipeline, above all a distributed one, is also walked step by step: that is step-debug.

In plan mode, debugging is refused: it comes after the user approves the plan (W-5).

### Step-debug

Step-debug is event-driven, step-by-step debugging of the project the agent works on, like an IDE's stepping debugger ([ADR 0013](decisions/0013-step-debug-mode.md)). It pays off for large pipelines spread over services, queues and agents, where no ordinary debugger spans the whole path.

While it is on, the agent:

1. draws a flowchart of each pipeline it debugs ([live flowcharts](maps.md#live-flowcharts));
2. instruments the project's code with callbacks into the harness: `notification` brings data to the agent, `journal` records the event, `flowstate_update` marks the chart's active node;
3. builds a **debug router** in the target project, so the pipeline takes its next step only when the agent allows it.

The harness pauses nobody's code: the stepping lives in the target project.

The debug router is always built by one contract, an OpenAPI document the harness ships (CON-27):

- an `approve` route lets the app go to its next step;
- a `step back` route exists only where stepping back is really possible, with no stubs;
- product routes call the functions the real code calls and implement no logic of their own (CON-28).

A waiting step advances only on the agent's `approve`: no timeout, no default action (CON-29). Everything the mode adds to the target project is active only with `MULTITOOL_DEBUG_MODE=1`, and the router only with authorization (CON-25). Without the gate the code is inert.

When a run reaches a terminal block of its flowchart, the cycle ends. It is saved to the project's `.harness/debug/runs.sqlite` and the chart is reset (CON-24).

### The status line

The status line shows the mode first, as the field MODE:

| MODE | When |
|---|---|
| `normal` | no mode is on |
| `plan` | plan mode |
| `debug` | debug mode |
| `orchestrate` | the conversation is an orchestrator |
| `slave` | the conversation is an agent of an orchestrator |

Several at once are joined with ` + `, as in `orchestrate + plan`. A new tab resets MODE (CON-105).

## Using it

| Tool | For |
|---|---|
| `mode__plan_enter` | enter plan mode; answers with its rules |
| `mode__plan_exit` | show the plan and ask to leave: `plan`, and `user_said` when the user has just said to go |
| `mode__debug` | debug mode on or off (`on`) |
| `debug_contract` | the step-debug router's contract, as an OpenAPI document (a utility) |
| `debug_runs` | saved step-debug runs, to compare (a utility) |

In the terminal UI ([the TUI](tui.md#planning-first)):

- `/plan` puts the session in plan mode; `/plan off` leaves it;
- `/stepdebug` turns step-debug mode on, `/stepdebug off` off.

From the command line:

- `multitool debug contract` prints the router's contract;
- `multitool debug check <url>` tests a router;
- `multitool debug runs` lists the saved runs.

## Settings

`session.plan_first` says what the built-in hook `mode-guide` does when the agent writes with no plan made in the session (CON-64):

- `remind` adds one reminder for one turn;
- `enforce` refuses the write and says why;
- `off` does nothing.

It never touches reads or memory, and cannot loop. `mode-guide` also says the step of the cycle that is due, for one turn, at each user message and each change of mode (CON-66).

## Limits and status

- On an agent backend, the harness cannot refuse that program's own writes as it does on its own loop: plan mode holds from its next message (CON-106).
- Step-debug code in a target project works only behind `MULTITOOL_DEBUG_MODE=1`, with authorization.
- A reliability-monitoring mode is a concept, not built: metrics come in, and each threshold or SLO event has a response policy ([ADR 0014](decisions/0014-reliability-monitoring-mode.md)).
- A brainstorming mode, with agents as peers over agent-to-agent messages, is planned in the [roadmap](spec/roadmap.md) and not built.

## Specs and decisions

- [ADR 0016](decisions/0016-plan-mode.md): plan mode
- [ADR 0013](decisions/0013-step-debug-mode.md): step-debug mode
- [ADR 0021](decisions/0021-agent-behaviour.md): plan first, then the mode the work needs
- [ADR 0012](decisions/0012-flowcharts-as-live-pipelines.md): live flowcharts

Specs:

- IF-0.5: session modes; P-10: a mode is a framework
- CON-96: modes are not utilities
- CON-64, CON-65, CON-66: `plan_first`, writes under `docs/`, `mode-guide`
- CON-106: modes on an agent backend
- CON-117: a plan holds a map and a chart
- CON-158, CON-161, CON-163: approval in the window and on Telegram
- CON-24 to CON-29: step-debug runs, gate, router contract, waits
- [Agent workflow](agent-workflow.md): W-1 to W-5, W-30 to W-36, W-41
