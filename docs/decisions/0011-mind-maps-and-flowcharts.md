# 0011 — Mind maps and flowcharts are separate tools

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-4, IF-7, IF-11

## Decision

Both document the work, but they answer different questions, so they are two tools with their own operations, not one tool with a type switch:

| | Mind map | Flowchart |
|---|---|---|
| Answers | *what* is known about a topic: a map | *how* something flows from one point to another |
| Shape | a tree around one topic, with cross links | a directed graph from start to end |
| Nodes | topic, goal, decision, fact, hypothesis, question, task, idea, dead end, note; open / done / dropped | ISO 5807 symbols: start, end, process, decision, input/output, subprocess, document, data, manual input, preparation, connector, delay, note; logic gates AND, OR, NOT, NAND, NOR, XOR, XNOR |
| Edges | parent → child branches; labelled cross links ("contradicts") | arrows with conditions ("yes", "no"), loops allowed |
| Typical use | working memory of a task, research, the project's architecture | a request path, an algorithm, a release procedure, the logic of a check |
| Operations | `mindmap_new`, `mindmap_add --parent`, `mindmap_link`, `mindmap_flow <node\|branch\|link> <flowchart>`, `mindmap_done` / `drop`, `mindmap_show` | `flowchart_new --direction down\|right`, `flowchart_add --after <node> --label <cond>`, `flowchart_link`, `flowchart_show` |

A mind map can point at flowcharts: any node, branch or cross link may link to one or more flowcharts (the map says *what*, the linked flowchart says *how* that part works). Shown as `▸ flow #N` on the node, and under the tree for a branch or a link.

Both live in `Memory` (IF-7), point their nodes at journal entries, are drawn by frontends in text and as SVG (journal#140), and show in the transcript as their own events (MINDMAP, FLOWCHART).

## Why

A flowchart forced into a mind map loses its direction and its symbols' meaning; a mind map forced into a flowchart becomes a false process. Separate tools let the agent pick by the question it is answering, and let each tool validate its own structure (a flowchart has a start, every arrow leads somewhere, a decision has labelled exits; a mind map is a tree).

## Context

The memory backend carried over (claudebase) has flowcharts as a `--type` of its mind maps (`claudebase mindmap new --type flowchart`, ISO 5807 symbols and gates, direction down or right). The harness keeps that model's content and splits the interface; the backend adapter maps the two tools onto it until the backend splits too.

## Rejected

- **One `map` tool with a type flag** (the backend's current shape): the agent has to know the switch exists and the operations mix (`--parent` means nothing in a flowchart, `--after` nothing in a mind map).

## Roles — clarified by the owner, 2026-11-01

- **A mind map is a document**: the map of the project's decisions, questions, constraints, invariants and specifications. When a developer asks why the system is made this way, the map of decisions, constraints and specifications must answer it.
- **A flowchart is both** a document of a flow and the **live view of step-debug mode** ([ADR 0013](0013-step-debug-mode.md)): the chart a pipeline's run walks through, step by step. That is what sets it apart from a mind map.
- **Both render to SVG files**, as claudebase does (`claudebase mindmap svg <id> -o map.svg`): a map or chart can be saved and shared as a picture outside the harness. A good practice the harness keeps.
