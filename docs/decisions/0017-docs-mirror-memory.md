# 0017 — Project documentation mirrors the journal, mind maps and flowcharts

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** CON-43, IF-6, IF-7, P-4, P-5; replaces the "plan file is optional" part of [ADR 0016](0016-plan-mode.md) (Q-26)

## Decision

Writing the documentation to files is **mandatory**, not an optional step. The journal, the mind maps and the flowcharts of a project are mirrored into Markdown files in the project's repository, and the two stay **consistent**: what the journal says is what the files say.

The shape is the one this repository uses for itself, which the owner chose as the model:

| Files | What they mirror |
|---|---|
| `README.md` | what the project is, its status, links to the documents |
| `docs/architecture/overview.md` | the architecture mind map and the main flows (diagrams, the layout of the code) |
| `docs/spec/principles.md` | the principles the project optimises for |
| `docs/spec/interfaces.md` | specification entries: behaviour, boundaries, contracts |
| `docs/spec/invariants.md` | invariants, each with how it is checked |
| `docs/spec/constraints.md` | constraints: rules, limits, constants |
| `docs/spec/open-questions.md` | open questions; settled ones struck through with the answer |
| `docs/spec/roadmap.md` | milestones and the plan: the journal's todos grouped into work |
| `docs/decisions/NNNN-*.md` + index | decisions, one record each, with status, who decided, why, what was rejected, and what amends or replaces it |
| flowcharts | a Markdown page per chart (or a section in the page of its feature) with the chart drawn as a diagram and exported to SVG |

**Consistency is part of the work.** A decision, specification, constraint, invariant or question that goes into the journal goes into its file in the same change; a reversal deprecates the journal entry and updates or strikes the file. A disagreement between the files and the journal or a map is a finding, settled before the work goes on. Ids (P-, IF-, INV-, CON-, Q-, ADR numbers) are stable and the same in both.

**Why both.** The journal, maps and charts are what the agent searches and keeps current; the files are what a person reads, reviews in a pull request and finds in the repository without the harness. Files alone go stale; the journal alone is invisible to people. Mirrored and kept consistent, each covers the other's weakness.

**Plan mode.** A plan made in plan mode ([ADR 0016](0016-plan-mode.md)) is mirrored the same way — its decisions, specifications and questions into their files, its work items into the roadmap — rather than saved as a separate optional plan file.

## How the mirror is kept — decided by the owner (answers Q-27)

**The agent writes the files; a hook makes sure it does.** After every tool call that writes to the journal, a mind map or a flowchart, the harness fires a `PostToolUse` hook (IF-6) whose condition is that write. The hook adds to the agent's context the instruction to update the documentation files that mirror what was written, naming the file:

| Write | File to update |
|---|---|
| a decision (new, amended, replaced) | its ADR in `docs/decisions/` and the index |
| a specification | `docs/spec/interfaces.md` (or the feature's spec page) |
| a constraint · an invariant · a principle | `constraints.md` · `invariants.md` · `principles.md` |
| a question opened · settled | `open-questions.md`: a new row · the row struck through with the answer |
| a todo · a closed todo | `roadmap.md` |
| an entry marked deprecated | the file that states it: updated, or the item marked replaced |
| a mind-map change to the architecture map | `docs/architecture/overview.md` |
| a flowchart change | the chart's page, its diagram and SVG |

Writes that mirror into no file — an update (the changelog is the journal and git), a finding, a lesson, a verification — fire no instruction (proposed by the agent).

**Proposed by the agent, to be confirmed:** a `Stop` hook at the end of the agent's turn checks that every write of the turn that needs a file has its file changed in the working tree, and reminds the agent of the ones that do not, so a forgotten update is caught before the turn ends (Q-28).
