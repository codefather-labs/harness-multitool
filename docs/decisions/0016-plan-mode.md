# 0016 — Plan mode (idea)

- **Status:** proposed — an idea of the project owner; nothing here binds until the owner accepts it · **Raised by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, IF-7; a sibling of [ADR 0013](0013-step-debug-mode.md) (step-debug mode) and [ADR 0014](0014-reliability-monitoring-mode.md) (reliability monitoring)

## The idea

The harness gets a **plan mode**: the classic plan mode of agent harnesses, where the agent studies the task and the code and writes a plan before it changes anything, the user approves the plan, and only then does work begin. Here the accent is on the harness's memory tools:

- the **journal** — every decision, finding, assumption and open question of the planning goes into the journal as it is made; the work items of the plan are journal todos, each with its test protocol (what to test, regressions included, how, under which conditions, what evidence proves it); the rules the work must follow are specifications and constraints;
- **mind maps** — the map of what is known about the task: decisions, questions, constraints, invariants, alternatives considered and rejected;
- **flowcharts** — the flows the plan changes or adds, drawn before they are built.

**A hook sets the agent's rules.** Entering plan mode fires a hook (IF-6) that adds to the agent's context the instructions for these tools: what to journal and how, when to open a mind map and what goes into it, when a flow deserves a flowchart. Leaving the mode is the user's approval of the plan.

## Proposed by the agent, to be confirmed

- **What plan mode allows.** Like every plan mode: reading, searching and running read-only commands, but no edits to the project, no commits, nothing with side effects outside the harness. Writing to the journal, mind maps and flowcharts *is* allowed, since that is the point of the mode; it changes the harness's memory, not the project.
- **The plan is the journal.** The approved plan is not only text in the conversation: it is the set of journal todos, specifications and decisions the planning wrote, plus its mind map and flowcharts, so the next session and other agents find it.
- **Switching.** The user switches the mode on, or the agent proposes it for a task that needs a plan; leaving it always takes the user's approval.

## Open

The proposals above (Q-25); whether a plan has a document of its own besides the journal entries, mind map and flowcharts (Q-26).
