# 0017 — Project initialization; documentation mirrors the journal, mind maps and flowcharts

- **Status:** accepted · **Decided by:** the project owner, 2026-11-01
- **Specs:** CON-43, CON-44, CON-45, CON-46, IF-6, IF-7, P-4, P-5; replaces the "plan file is optional" part of [ADR 0016](0016-plan-mode.md) (Q-26)

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

**The docs are a storefront: no drafts (the owner, 2026-11-01).** The files hold only the current, settled state of the project as it is meant to be read: no drafts, no work notes, no scratch text, no TODO placeholders, no half-written sections, no superseded variants kept "for history", and no assumption written as if it were a fact. Work in progress lives in the journal, mind maps and flowcharts; git keeps the history. What is not settled appears only in its declared form — an open question in `open-questions.md`, an ADR with the status `proposed` or `concept`, a proposal labelled as one — never as unfinished text (CON-46).

**Consistency is part of the work.** A decision, specification, constraint, invariant or question that goes into the journal goes into its file in the same change; a reversal deprecates the journal entry and updates or strikes the file. A disagreement between the files and the journal or a map is a finding, settled before the work goes on. Ids (P-, IF-, INV-, CON-, Q-, ADR numbers) are stable and the same in both.

**Why both.** The journal, maps and charts are what the agent searches and keeps current; the files are what a person reads, reviews in a pull request and finds in the repository without the harness. Files alone go stale; the journal alone is invisible to people. Mirrored and kept consistent, each covers the other's weakness.

**Plan mode.** A plan made in plan mode ([ADR 0016](0016-plan-mode.md)) is mirrored the same way — its decisions, specifications and questions into their files, its work items into the roadmap — rather than saved as a separate optional plan file.

## Project initialization

Not every project needs a `docs/` tree, so it is created by a **skill of the agent**, `project-init` (IF-14), not unconditionally at the harness's start (the owner, 2026-11-01). Initializing a project creates both of the harness's folders in it: the **`docs/` tree** and the **`.harness/` folder** (the owner). The skill creates the whole docs hierarchy at once, each file with an example entry showing how it is written — the shape of this repository:

```text
README.md                       what the project is, status, links (left alone if it exists)
docs/
  architecture/overview.md      layers diagram, one main flow, repository layout — example nodes
  spec/README.md                how the spec is organised, the id scheme
  spec/principles.md            P-1 example
  spec/interfaces.md            IF-1 example: contract and boundaries
  spec/invariants.md            INV-1 example with "Checked by"
  spec/constraints.md           sections numbered by tens, CON-1 example
  spec/open-questions.md        Q-1 open, Q-2 settled (struck through, with the answer)
  spec/roadmap.md               M0 — Specification, with "Done when"
  decisions/README.md           the index and the status values
  decisions/0001-record-decisions.md   the first ADR: decisions are recorded here
.harness/
  README.md                     what the folder holds
  policies/                     response policies of reliability monitoring (CON-26)
  debug/                        step-debug runs, SQLite (CON-24)
```

Existing files are never overwritten; the skill fills only what is missing. Once the tree exists, the mirror rule (CON-43) and its hook apply to the project.

**Asked once (answers Q-29, decided by the owner).** At the first start in a project with no `docs/` and no `.harness/`, the harness asks once whether to initialize the project. A project that says no is not mirrored until `project-init` is run; the question is not asked again. The agent's addition: a mode that needs a `.harness/` subfolder (step-debug's `debug/`, monitoring's `policies/`) creates just that subfolder when it is first used, so a project that declined the docs can still use the modes.

**Everything in `.harness/` is committed (answers Q-30, decided by the owner).** For now the whole folder goes into the repository, step-debug runs included, and `project-init` writes no `.gitignore` lines; if something comes up in use, the policy changes.

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

**The consistency check — decided by the owner (answers Q-28).** A check that cannot break anything and needs no workarounds:

1. **A built-in hook plugin on typed events.** A write to the journal, a mind map or a flowchart is a typed event on the bus (IF-0.2) with its category and flags; the check is a hook plugin (IF-6) that reads those events directly — no parsing of shell output or tool text.
2. **Linked by id.** A write that must be mirrored carries the id of its documentation item — P-, IF-, INV-, CON-, Q- or ADR number, the same in both places (CON-43). The check is deterministic: the file the write's category names contains that id; a settled question is struck through with its answer; a deprecated or replaced item is marked so.
3. **Advisory, once.** A `Stop` hook at the end of the agent's turn adds **one** reminder to the next turn listing the items that fail the check. It never vetoes, never forces the agent to continue, and so cannot loop; a hook that fails is logged and ignored.
4. **The same checker as a command and in CI** — `multitool docs check` — so drift is caught even when the hook was off or the reminder was ignored.

**Limit.** The check covers structure — ids present, statuses matching — not that the prose of a file says what the journal entry says. That agreement stays the agent's work, or a periodic review agent's (CON-45).
