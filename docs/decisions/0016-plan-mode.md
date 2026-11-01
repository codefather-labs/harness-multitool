# 0016 — Plan mode (idea)

- **Status:** proposed — an idea of the project owner; the parts marked decided bind, the rest is open · **Raised by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, IF-7; a sibling of [ADR 0013](0013-step-debug-mode.md) (step-debug mode) and [ADR 0014](0014-reliability-monitoring-mode.md) (reliability monitoring)

## The idea

The harness gets a **plan mode**: the classic plan mode of agent harnesses, where the agent studies the task and the code and writes a plan before it changes anything, the user approves the plan, and only then does work begin. Here the accent is on the harness's memory tools:

- the **journal** — every decision, finding, assumption and open question of the planning goes into the journal as it is made; the work items of the plan are journal todos, each with its test protocol (what to test, regressions included, how, under which conditions, what evidence proves it); the rules the work must follow are specifications and constraints;
- **mind maps** — the map of what is known about the task: decisions, questions, constraints, invariants, alternatives considered and rejected;
- **flowcharts** — the flows the plan changes or adds, drawn before they are built.

**A hook sets the agent's rules.** Entering plan mode fires a hook (IF-6) that adds to the agent's context the instructions for these tools: what to journal and how, when to open a mind map and what goes into it, when a flow deserves a flowchart. Leaving the mode is the user's approval of the plan.

## Decided by the owner (2026-11-01)

- **What plan mode allows.** The hook tells the agent: only the harness's tools (journal, mind maps, flowcharts), read operations (reading, searching, read-only commands) and **web search** are available now. No edits to the project, no commits, nothing with side effects. It also sets the mindset: concentrate on planning, do not get distracted by anything else.
- **The plan is the journal.** The approved plan is not only text in the conversation: it is the set of journal todos, specifications and decisions the planning wrote, plus its mind map and flowcharts, so the next session and other agents find it.
- **A framework, not instructions.** Where plan mode elsewhere is a prompt, here it is a framework: documented tools, attention to keeping the documentation current, spec-first (P-4, P-10).

- **Switching (answers Q-25).** Both ways, as in other harnesses: the user switches the mode on, or asks the agent to plan something and the agent enters plan mode itself. Leaving it takes the user's approval of the plan.
- **A plan file is optional (answers Q-26).** The plan lives in the journal, mind maps and flowcharts, which the agent always has at hand and keeps current. Saving the plan to a file is an optional last step, only when the human wants it, once everything is planned and recorded: files tend to remain as stale leftovers, yet they matter to people. This part is not fully thought through yet and may change.

## Open

The optional plan file: its form and where it goes (the owner: not fully thought through yet).
