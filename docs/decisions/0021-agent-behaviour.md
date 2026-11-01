# 0021 — How the agent behaves: plan first, then the mode the work needs

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, CON-62, CON-63, CON-64; builds on [ADR 0016](0016-plan-mode.md); amends [ADR 0013](0013-step-debug-mode.md)

## Decision

The harness specifies how its agent behaves, because a model will not reach for a mode by itself. Asked to try the tools on a small to-do app, a real model listed `plan_enter` when asked, and yet went straight to writing files. Switching a mode matters: it is what gives the agent that mode's instructions, and reminds it of the tools.

- **Plan first.** Building an app, a prototype or a project, a feature, a fix, or any change that is not trivial starts with `plan_enter`, even if nobody said "plan" and even if the result was asked for directly. "Let's try" or "make a small app" state an intent; they do not approve a plan nobody has proposed yet. Answering, reading, diagnosing with no changes, and work inside an approved plan are exempt.
- **In plan mode:** study, settle what is ambiguous, journal the decisions, make each work item a todo with its test protocol, map and chart what changes, mirror into the docs if the project has them; then `plan_exit`, and nothing is built before approval.
- **After the plan.** A planned fix goes on in debug mode: reproduce, change, verify. A planned feature goes on in normal mode. Work ends with evidence, not a claim.

**This amends ADR 0013:** it kept step-debug away from small tasks because instrumenting costs more than it gives. The mode is now entered for every planned fix, for its instructions; only a pipeline gets a router and instrumentation, and a small bug is reproduced by a failing test.

## How the harness holds to it

The rules are in every session's system prompt, and `plan_enter`'s description says when to use it. A built-in hook, `mode-guide`, watches for the first write in a message when no plan has been made in the session. `session.plan_first` says what it does: `remind` adds one reminder for one turn, `enforce` refuses the write and says why, `off` does nothing. It never touches reads or memory, and cannot loop.

Whether a task needs a plan stays the model's judgment under these rules: the harness does not classify a user's words.
