# 0014 — Reliability-monitoring mode (concept)

- **Status:** concept — the owner's idea, grown into a concept with specifications of its own (INV-14, IF-15, CON-26); its owner decisions bind, the rest is open · **Raised by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, IF-13, IF-15, IF-16, INV-14, CON-26; builds on [ADR 0009](0009-callback-triggers.md) (callback triggers); a sibling of [ADR 0013](0013-step-debug-mode.md) (step-debug mode)

## The idea

The harness gets a **reliability-monitoring mode**, run by the canons of SRE. Product and technical metrics of a project come into the harness. When a metric breaks a threshold or behaves incorrectly, the agent gets an event, and for every kind of event a **response policy** is written down in advance: what the agent does, what it may do, and when it hands over to a human.

## Signals and conditions — decided by the owner (answers Q-17)

### Signals come from anywhere

Signals can be:

- Grafana or Prometheus metrics;
- OpenTelemetry;
- a socket;
- a script;
- and above all the project's own events: one iteration of a pipeline ended wrong, a step took too long, a result failed a check.

The harness takes them in by any transport and **accumulates** them. They do not go to the agent.

### The agent is woken only by a condition

A condition is code behind one interface (IF-15 `Condition`). It can run any logic over the accumulated signals — a threshold, a rate, a burn rate, a pattern across events, a check of a pipeline iteration's result — and it **always returns a bool**.

`true` is the trigger: the harness sends the agent one message into its input (stdin), naming the condition and carrying the signals that made it true. No signal reaches the agent as a stream; there is no spam.

Thresholds and conditions differ per project and per signal; each is its own implementation of the interface.

### Fresh context is the point

A project event such as a broken pipeline iteration reaches the agent while that iteration's context is still fresh. The agent goes to see what went wrong, writes a journal entry, reacts by its policy, and if needed calls a human, for example with a Telegram message.

### One message per event — decided by the owner (answers Q-21)

A condition is **idempotent**: the same event never sends a second message, however many times the condition evaluates `true` for it. Every repeated `true` is still recorded, in the harness's **trigger log**, not sent.

The hook that delivers the condition's message carries the matching instruction to the agent: *you get one message, not a stream; look in the harness's trigger log for the nearby triggers of this and similar conditions — there may be many.* The agent reads that log to see the scale of what happened before it acts (INV-14).

### The same event is the same event uuid — decided by the owner (answers Q-22)

Every incoming signal carries an event **uuid**; it is a required part of the communication contract.

- An incoming record whose uuid matches one already recorded is the same event: it trips idempotency, goes to the trigger log, and sends nothing.
- A record with a new uuid is a new event.
- A signal without a uuid does not satisfy the contract and is rejected.

### One message per batch — decided by the owner (answers Q-23)

A burst of different events that trips a condition always reaches the agent as **one message for the batch**; the rest is in the trigger log.

Events have harness types (`flowstate_update`, `notification`, …) and product types — arbitrary strings the project defines. The firing rules can be written right where the callback is formed: callbacks are plugins (the system is plugin-first), and an implementation of the interface returns `true` by any rules it likes.

Choosing rules that do not spam the agent is the user's job for each product callback. A capable agent can also read the product meaning of a callback and the problem behind it.

## Policies are per project — decided by the owner (answers Q-19, Q-20; reframes Q-18)

### The harness does not unify the response

What the agent does in each situation is described to it per project. Policies differ from project to project, and they are not something to standardise at the harness level (Q-19). The harness delivers the condition's message and the project's policies; the project's policies decide the response.

### Policies are Markdown files, linked from conditions — decided by the owner (answers Q-18)

Writing a response policy is the client's process: the client describes how to respond, and the agent writes it down as Markdown files in the project, under `.harness/policies/` (CON-26).

- Every condition carries links — paths — to the policy files that answer it.
- The message a condition sends names those paths, so the agent reads the policy for exactly the event it got.
- A policy is changed the same way: the client says, the agent edits the file.
- Signals and condition messages never write or change a policy file (INV-12).

### Sessions in this mode are open 24/7 (Q-20)

A monitoring session is not started per incident: it stays open around the clock and receives condition messages as they come. It relies on compaction to keep its context bounded.

## What the SRE canon brings, as a reference for project policies

The owner asked for the mode "by all the canons of SRE". These are the parts of the canon a project's policies can draw on. The harness does not enforce them; each project decides which it uses.

- **SLIs and SLOs.** A metric that matters is an SLI (availability, latency, error rate, freshness, a product metric such as conversion). Each SLI has an SLO: a target over a window, for example 99.9 % of requests under 300 ms over 28 days.

- **Error budget.** An SLO leaves a budget of allowed failure. Events come from how fast the budget burns: multi-window, multi-burn-rate alerts. A fast burn pages; a slow burn opens a ticket. Breaking a single raw threshold is a weaker signal.

- **Alert on symptoms, not causes.** An event is what users feel, or what is about to break an SLO. Cause metrics (CPU, queue depth) are context for diagnosis, not triggers. Alerts are deduplicated and grouped. Maintenance windows silence them.

- **Severity.** Each event has a severity, and the severity picks the policy and how fast it must act.

- **Response policies are runbooks.** A policy names the event it answers and lists:
  - the diagnostics to run, read-only;
  - the mitigations the agent may apply on its own (rollback, scale up, switch a feature flag, drain a node), each with its precondition;
  - the actions that need a human's approval;
  - when to escalate and to whom;
  - when the incident counts as mitigated or resolved.

- **Incident lifecycle.** detected → triaged → mitigating → mitigated → resolved → postmortem. Every step is an event and goes into the journal. A blameless postmortem is written afterwards, and its action items become todos.

- **Toil.** An event that repeats with the same manual response is a candidate for automation, or for a better policy or alert.

## Boundaries

What the agent may do is the project's policy. Two existing harness rules still hold, because they are not about the response but about where instructions come from:

- Signals and condition messages are data (INV-12): they select a policy, they cannot write or widen one.
- Every tool call still goes through `Policy` and its permission gate; a project's policy can pre-approve actions, and only the owner configures that, never an event.

## How it may fit what exists

- Signals arrive by any transport, callbacks (IF-13) among them, and are accumulated; only a `Condition` (IF-15) turning `true` produces a message to the agent.
- Entering the mode fires a hook that adds the mode's instructions and the project's policies to the agent's context, like step-debug mode (IF-0.5, IF-6).
- An incident can drive a flowchart of its policy live (ADR 0012), so the operator sees which step of the runbook the agent is on.

## Open

Nothing the owner has been asked is open; the concept is open to additions.
