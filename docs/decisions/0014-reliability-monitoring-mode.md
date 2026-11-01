# 0014 — Reliability-monitoring mode (idea)

- **Status:** proposed — an idea of the project owner, not yet a design; nothing here binds until the owner accepts it · **Raised by:** the project owner, 2026-11-01
- **Specs:** IF-0.5, IF-6, IF-13, IF-15; builds on [ADR 0009](0009-callback-triggers.md) (callback triggers); a sibling of [ADR 0013](0013-step-debug-mode.md) (step-debug mode)

## The idea

The harness gets a **reliability-monitoring mode**, run by the canons of SRE. Product and technical metrics of a project come into the harness. When a metric breaks a threshold or behaves incorrectly, the agent gets an event, and for every kind of event a **response policy** is written down in advance: what the agent does, what it may do, and when it hands over to a human.

## Signals and conditions — decided by the owner (answers Q-17)

**Signals come from anywhere.** Grafana or Prometheus metrics, OpenTelemetry, a socket, a script, and above all the project's own events: one iteration of a pipeline ended wrong, a step took too long, a result failed a check. The harness takes them in by any transport and **accumulates** them. They do not go to the agent.

**The agent is woken only by a condition.** A condition is code behind one interface (IF-15 `Condition`): it can run any logic over the accumulated signals — a threshold, a rate, a burn rate, a pattern across events, a check of a pipeline iteration's result — and it **always returns a bool**. `true` is the trigger: the harness sends the agent one message into its input (stdin), naming the condition and carrying the signals that made it true. No signal reaches the agent as a stream; there is no spam. Thresholds and conditions differ per project and per signal; each is its own implementation of the interface.

**Fresh context is the point.** A project event such as a broken pipeline iteration reaches the agent while that iteration's context is still fresh: the agent goes to see what went wrong, writes a journal entry, reacts by its policy, and if needed calls a human, for example with a Telegram message.

**Proposed by the agent, to be confirmed:** a condition that stays `true` must not resend on every evaluation. The agent proposes that the harness sends on the rising edge only (`false` → `true`), plus an optional cool-down per condition, so a long outage is one message, and a new one comes after it has recovered (Q-21).

## What the SRE canon brings, as a starting point

The owner asked for the mode "by all the canons of SRE". These are the parts of the canon the agent proposes to build on. All of them are open until the owner decides.

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

## Boundaries the agent proposes

- Metrics and alerts are data (INV-12). They select a policy; they cannot write or widen one.
- A policy can only narrow what `Policy` allows (IF-14 style). An action that changes a production system goes through the permission gate unless the policy explicitly pre-approves it, and that pre-approval is configured by the owner, never by an event.
- The agent never silences an alert or changes an SLO on its own.

## How it may fit what exists

- Signals arrive by any transport, callbacks (IF-13) among them, and are accumulated; only a `Condition` (IF-15) turning `true` produces a message to the agent.
- Entering the mode fires a hook that adds the mode's instructions and the project's policies to the agent's context, like step-debug mode (IF-0.5, IF-6).
- An incident can drive a flowchart of its policy live (ADR 0012), so the operator sees which step of the runbook the agent is on.

## Open

The edge-and-cool-down rule for conditions (Q-21); the format and storage of SLOs and policies (Q-18); which actions an agent may take on its own and how humans are paged (Q-19); how a mode that runs for days fits a session (Q-20).
