# 0021 — How the agent behaves: plan first, then the mode the work needs

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, CON-62, CON-63, CON-64; builds on [ADR 0016](0016-plan-mode.md); amends [ADR 0013](0013-step-debug-mode.md)

## Decision

The harness specifies how its agent behaves, because a model will not reach for a mode by itself. Asked to try the tools on a small to-do app, a real model listed `mode__plan_enter` when asked, and yet went straight to writing files.

Switching a mode matters: it is what gives the agent that mode's instructions, and reminds it of the tools.

- **Plan first.** Building an app, a prototype or a project, a feature, a fix, or any change that is not trivial starts with `mode__plan_enter`. This holds even if nobody said "plan" and even if the result was asked for directly. "Let's try" or "make a small app" state an intent; they do not approve a plan nobody has proposed yet.
  - Exempt: answering, reading, diagnosing with no changes, and work inside an approved plan.

- **In plan mode:** study, settle what is ambiguous, journal the decisions, make each work item a todo with its test protocol, map and chart what changes, mirror into the docs if the project has them; then `mode__plan_exit`, and nothing is built before approval.

- **After the plan.** A planned fix goes on in debug mode: reproduce, change, verify. A planned feature is built and run, then debugged in debug mode the same way (the owner, 2026-11-01, replacing "goes on in normal mode"). Work ends with evidence, not a claim.

**This amends ADR 0013:** it kept step-debug away from small tasks because instrumenting costs more than it gives. The mode is now entered for every planned fix, for its instructions; only a pipeline gets a router and instrumentation, and a small bug is reproduced by a failing test.

## How the harness holds to it

The rules are in every session's system prompt, and `mode__plan_enter`'s description says when to use it.

A built-in hook, `mode-guide`, watches for the first write in a message when no plan has been made in the session. `session.plan_first` says what it does:
- `remind` adds one reminder for one turn;
- `enforce` refuses the write and says why;
- `off` does nothing.

It never touches reads or memory, and cannot loop.

Whether a task needs a plan stays the model's judgment under these rules: the harness does not classify a user's words. The rules themselves are now one document, [the agent workflow specification](../agent-workflow.md), which ships with the harness (CON-97, CON-98).

A user's own words can approve a plan: the agent quotes them in `mode__plan_exit`, and the harness checks they are in the user's last message.

## The whole cycle, held by reminders

The owner tried the cycle on a small local model. It entered plan mode unprompted, then asked nothing, recorded nothing and retried refused writes.

A long system prompt does not carry a small model through a long run, so `mode-guide` says each step when it is the step at hand (CON-66):

- at every user message, and on entering plan mode, leaving it, and entering debug mode, the one step that is due, for one turn;
- a `mode__plan_exit` with no journal entry, map or flowchart made while planning goes back once, naming what is missing, and the user is not asked yet;
  - under `enforce`, it goes back until they exist;
  - what a map and chart hold is CON-117, which refuses always;
- stopping after an approved plan, or in debug mode, makes the run go on once per message, with that step said again.

While planning, `docs/` may be written, because a feature is described before it is built; everything else stays refused (CON-65). `plan_first = off` turns all of this off.

A small model reaches for tools and does not end a turn by itself, so a question is a tool: `ask_user` shows it and ends the run (CON-67).

And it repeats what fails. The same call failing the same way gets a reminder the third time, and ends the run with a visible error the fifth, rather than spending the budget in silence (CON-68).
