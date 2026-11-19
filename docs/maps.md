# Mind maps and flowcharts

Mind maps hold where a long task stands; flowcharts draw how something flows. Both are kept by the daemon beside the project's [journal](journal.md), and their nodes point at its entries.

## What it is

They are two separate tools, because they answer different questions ([ADR 0011](decisions/0011-mind-maps-and-flowcharts.md)):

| | Mind map | Flowchart |
|---|---|---|
| Answers | *what* is known about a topic | *how* something flows from a start to an end |
| Shape | a tree around one topic, with cross links | a directed graph, loops allowed |
| Edges | parent to child; labelled cross links ("contradicts") | arrows with conditions ("yes", "no") |
| Typical use | the working memory of a task, a research, a hunt for a cause, the project's architecture | a request path, an algorithm, a release procedure, the logic of a check, a pipeline |

A mind map is a document: the map of a project's decisions, questions, constraints and specifications, which answers why the system is made this way. A flowchart is both a document of a flow and the live view of a running pipeline (below).

## How it works

### Mind map nodes

A mind map is a tree from its root, the topic. Its node kinds, as the terminal UI draws them:

| Kind | Symbol |
|---|---|
| topic (the root) | ◆ |
| goal | ◎ |
| decision | ◈ |
| fact | ● |
| hypothesis | ◐ |
| question | ? |
| task | ☐ / ☑ |
| idea | ✦ |
| dead end (`dead-end`) | ✕ |
| note | ✎ |

A node is open, done (drawn muted) or dropped (struck through). A dropped node stays visible, so what was ruled out is not tried twice.

A node, a branch or a cross link can point at flowcharts, shown as `▸ flow #3`: the map says what, the chart says how that part works.

### Flowchart symbols

A flowchart is a block diagram by ISO 5807. Its kinds in `flowchart_add`:

- symbols: `start`, `end`, `process`, `decision`, `io` (input/output), `subprocess`, `document`, `data` (data store), `manual` (manual input), `prep` (preparation), `connector`, `delay`, `note`;
- logic gates: `and`, `or`, `not`, `nand`, `nor`, `xor`, `xnor`, drawn as IEC symbols, `o` marking an inverted output.

A chart runs `right` (left to right) or `down` (top to bottom), chosen at `flowchart_new`. Each symbol is added with an arrow from the node `after`, and `label` is that arrow's condition.

### Refs to the journal

Any node can point at journal entries with `refs` (on `mindmap_add`, `flowchart_add`, `map_link`) or later with `map_ref`. The map holds where the work is; the journal holds the detail.

These refs join maps and journal entries into one graph of the project's knowledge. A mind map node shows its refs as `#137`; a flowchart node lists them on its last line, an arrow on its pill.

## Using it

| Tool | For |
|---|---|
| `mindmap_new`, `flowchart_new` | start a map or a chart (`title`; a chart's `direction`) |
| `mindmap_add` | add a node under `parent` (the root by default), with its `kind` and `refs` |
| `flowchart_add` | add a symbol with an arrow from `after`, its `label` and `refs` |
| `mindmap_show`, `flowchart_show` | read one back as an outline, and draw it for the user |
| `map_edit` | change a node's text, kind, or both |
| `map_node_state` | mark a node `open`, `done` or `dropped` |
| `map_link` | an arrow between two nodes, with a label and `refs` |
| `map_ref` | point a node at journal entries |
| `map_close` | close a map whose task is done; it stays readable |

Each tool is described in [Tools and modes](tools.md#maps). On an agent backend they are `mcp__harness__mindmap_new` and so on.

### In the terminal UI

Each result is a MINDMAP or FLOWCHART card in the transcript ([the TUI](tui.md#maps)). One layout draws both: filled cards and box-drawing lines in text mode, SVG in graphics terminals. Every map reads in text mode, which is the primary target.

A map wider than its room pans: press and drag on it. `◀ ▶` mark the hidden parts. There is no zoom.

### Tickets

A click on a ref unfolds the entries as a ticket on the right of the map card:

- on a mind map, Ctrl+click a `#N` ref;
- on a flowchart, click a node or an arrow pill that has refs.

In a ticket of several entries, a click on an entry's header shows it and folds the others. A second click, or a click on the ticket, folds it. Only one ticket is open in a transcript, and tickets do not come back after a `/reload`.

### Live flowcharts

A flowchart can be bound to a real process of the project: a CI job, a deploy, a data pipeline, a request path ([ADR 0012](decisions/0012-flowcharts-as-live-pipelines.md)). The process sends `flowstate_update` callbacks naming the chart, the node and its state, and the chart is drawn with that node active.

The chart the agent draws is the **reference**. Each pass of the process is an **instance** of it, keyed by the client's own `instance` or `correlation_id`. A `flowstate_update` changes only its instance, never the reference, and starts no agent turn (CON-23).

How instances draw ([the TUI](tui.md#standard-types)):

- one instance: a new card for each event;
- several: one row each, updated in place, collapsed by default and pulsing softly for a second on an update;
- Ctrl+G opens or closes the newest instance, and a click on a FLOW row opens that one.

The node a run is at is outlined in the accent; the node an event changed is tinted with it.

A flowstate payload is data: it cannot change the chart. A step that is not on the chart is recorded as off-chart, never added (INV-12).

### In plan mode

A plan holds both forms ([agent workflow](agent-workflow.md#4-in-plan-mode), W-33 and W-34):

- a mind map of the strategy: its decisions, its work and the proofs it will produce, with `refs` to journal entries;
- a flowchart of the logic branches of the planned behaviour, with its outcomes.

`mode__plan_exit` refuses a plan whose map or chart is empty (CON-117). A slave's mind map also points at its task (CON-111). See [Modes](modes.md#plan-mode).

A long task or a hunt for a cause keeps a mind map too (W-48): where it is, what it suspects, what is ruled out.

## Settings

**Settings > Memory** sets `map_max_nodes`: the most nodes a mind map or flowchart holds (CON-114). It is 200 by default, and 0 means no limit.

A session already running keeps the limit it started with until it restarts.

## Limits and status

- Past `map_max_nodes`, `mindmap_add` and `flowchart_add` are refused, naming the count and the limit.
- Export of maps and charts to SVG files is decided but not built yet ([ADR 0011](decisions/0011-mind-maps-and-flowcharts.md)).
- ADR 0012 also describes run history: runs listed, replayed and compared, with off-chart and stalled runs shown. How payloads map onto nodes and how long history is kept are open questions (Q-12, Q-13).
- Automations fired at a chart's node are an idea for later, not part of the decision.

## Specs and decisions

- [ADR 0011](decisions/0011-mind-maps-and-flowcharts.md): mind maps and flowcharts are separate tools
- [ADR 0012](decisions/0012-flowcharts-as-live-pipelines.md): flowcharts as live pipelines
- [ADR 0013](decisions/0013-step-debug-mode.md): step-debug mode walks a live chart
- IF-7: the `Memory` interface; IF-11: how frontends draw live charts
- CON-23: `flowstate_update` callbacks
- CON-114: `map_max_nodes`
- CON-117, CON-111: maps in a plan
- INV-12: a callback's payload is data
- Q-12, Q-13: open questions on live charts
