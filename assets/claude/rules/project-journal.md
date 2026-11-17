# Project Journal

Every project has a journal: the decisions, findings and updates of every earlier session and agent. It is two things at once — the record of **why** the project is the way it is (decisions and findings, so the next session does not re-decide or re-discover them) and the project's **changelog** (updates: what changed, when, where). Read it before you decide, write to it as you work.

## Where it lives and whose it is

- The multitool harness's daemon owns every journal, one per project. You reach it only through the harness's tools (`journal_*`, `mindmap_*`, `flowchart_*`, `map_*`; on an agent backend they are `mcp__harness__journal_add` and so on). Never open, copy or edit the journal files yourself.
- The project is the repository of the folder the session runs in (its git remote), or that folder itself when it has no remote — so every checkout of a repository shares one journal. Every tool works on that project's journal only.
- If a journal tool answers that the daemon is not reachable, tell the operator once and carry on without journaling; do not work around it.

## Read — before you decide

On a fresh context window (session start, resume, after compaction), and before any non-trivial decision:

- `journal_agenda` — what is open: todos, questions, choices waiting for the user's decision (`wait-for-decision`), problems, and the specifications and constraints that bind the work;
- `journal_tags` — the tag vocabulary, most used first;
- `journal_search` — by words, ranked; its `linked` names the neighbours of what it found;
- `journal_list` — by category, tags, flags, feature, open or closed, newest first;
- `journal_get` — one entry in full, with its links.

Read by category and tag, not by paging through everything.

An entry you rely on is evidence: cite it as `journal#<id>`. An entry is what an earlier session believed — if the code now says otherwise, the code wins, and that contradiction is itself a finding to journal.

## Write — as you work

Journal **every decision you make, every finding you uncover and every update you make**, one entry each, when it happens — not in a batch at the end, where the details are already gone.

`journal_add` with `body`, `category`, `tags` (1–4), and as needed `flags`, `salience` (high|medium|low), `feature`, `proof` and `proof_file`, `blocked_by`.

What counts:

- **decision** — a choice between alternatives, made by the operator or by you; every one gets its own entry: who decided, what was chosen, why, what was rejected and why.
- **finding** — something learned about the code, a system, a tool or the domain that was not obvious: a cause found, a constraint discovered, a behaviour verified.
- **bug** — a defect found: symptom, cause, where, whether fixed.
- **assumption-falsified** — something believed that turned out wrong, and what is true instead.
- **lesson** — a correction from the operator, or a mistake of your own, worth not repeating.
- **update** — a change you made to the project: code, config, docs, dependencies, infrastructure. What changed and where (files, the commit hash once there is one), and why — cite the `journal#<id>` of the decision behind it when there is one. These entries are the changelog.

Reuse the categories and tags the journal already has before inventing new ones.

How to write one:

- **Self-contained.** A later session reads it without this conversation: name the component, the file (`path:line`), the command, the version.
- **The why, not only the what.** "Chose WAL mode because concurrent readers blocked the daemon's writer (journal.rs:open); rejected a write queue as more moving parts" — not "switched to WAL".
- **Tags are how it is found again.** 1–4 specific nouns — component, technology, feature (`sqlite`, `daemon`, `telegram`, `install`); reuse existing tags rather than near-synonyms.
- **Salience is retention.** `high` — forever, for load-bearing decisions and constraints; `medium` (default) — a year; `low` — 90 days, for ambient context. Do not mark everything `high`.
- **Reversals cite what they reverse.** "Replaces journal#42: …".

## Work branches — every feature and fix, from todo to evidence

Every feature or fix you implement runs as a branch of the journal graph, in this order:

1. **Before you start: a todo with its test protocol** (`journal_add`, flag `todo`). What will be done and why, and — written now, while planning, not after the code exists — the **test protocol** that will prove it:
   - **what to test after the change**: the fix or feature itself, and the **regressions** — the neighbouring behaviour it can break, named one by one;
   - **how**: the commands, tests, live checks or scenarios to run;
   - **under which conditions**: environment, machine, data, load, how many runs;
   - **what evidence proves it**: the output, number, log line or screenshot each check must show to count as passed.

   When the result can only be read from **physical metrics of a long pipeline** (latency, throughput, memory, error rate, accuracy, a run's duration or cost), measure them **before** the change, under the conditions the protocol names, and record the baseline in the todo or in an entry of its own. The evidence entry then gives the same metrics **after**, measured the same way, next to the baseline.
2. **After implementing: the implementation, unverified** (`journal_add`, category `update`, flag `unverified`): "Implements journal#T: <what changed, where, commit>. Verification protocol: <steps and expected results>".
3. **After running the protocol: the evidence, verified** (`journal_add`, category `verification`, flag `verified`): "Verifies journal#I: <what was run, its output, what it proves>", with `proof` (a test's name, a command and its result, a commit, a URL) or `proof_file` (a file under the project's `.harness/proofs/`). `unverified` is lifted only by such a separate entry; never flag `verified` without having run the checks.
4. **Then close the branch:** `journal_close` the todo. It refuses until its branch holds a `verified` entry; when it closes, the open work items of the whole branch close with it.

Rules for the branch:
- `journal_branch` shows the whole branch from any of its entries.
- **What belongs to a branch:** only `Implements` / `Verifies` / `Fixes` links make an entry part of a branch, and it belongs to its nearest todo. Other mentions cite: they connect branches into a graph without merging them.
- **Evidence or nothing.** If the protocol fails, the branch stays open. Fix the code, add a new implementation entry, verify again.
- **Work that is dropped** closes with `journal_abandon`, saying why.

**A bug is a branch too.** Every bug you find gets its own entry the moment you find it: category `bug`, flag `todo`, with its test protocol. The fix is a separate entry, "Fixes journal#B: … Verification protocol: …" flagged `unverified`. Prove it, then "Verifies journal#F: <what was run, its output>" flagged `verified`, and `journal_close` the bug.

**Blocked work:** when something cannot be solved until another entry is, set `blocked_by` to that entry and work on the blocker first. A close that sets entries free names them as `released`; the sessions that wrote them are told.

## Links — what an entry intersects

Every `journal#N` in an entry's text is a link that both entries show. The verb just before it sets the kind: "Replaces journal#42" (also supersedes / corrects / reverses), "Fixes journal#26", "Implements journal#T", "Verifies journal#I", anything else cites. Link entries whose text does not mention each other with `journal_link`.

- Before relying on an entry, read its links (`journal_get`). A `replaced by` names the current word, and a neighbour flagged `deprecated` is not to be trusted as is.
- A link never changes a flag. When you replace an entry, mark the old one `deprecated` yourself with `journal_mark`.

## Flags — how far to trust an entry, and what is still open

Any entry can carry flags, any number at once:

| Flag | Meaning | Ends with |
|---|---|---|
| `todo` | something to do | `journal_close` |
| `question` | an open question | `journal_close` (the answer) |
| `wait-for-decision` | waits for a choice only its decider makes, usually the user, with the options laid out; the work that depends on it does not go on | `journal_close` (the decision) |
| `problem` | an unresolved problem | `journal_close` (the fix) |
| `unverified` | a hypothesis or an implementation not yet checked | `journal_close`, only once a separate `Verifies journal#N` entry carries the evidence |
| `specification` | how something must behave; binding | `journal_mark` remove (or replace it) |
| `constraint` | a rule, a limit, a constant or an invariant to follow; binding | `journal_mark` remove |
| `invariant`, `principle` | mirrored to `docs/spec/invariants.md` / `principles.md`, with their doc id | `journal_mark` remove |
| `important` | keep in view | `journal_mark` remove |
| `idea`, `concept`, `comment` | thinking, not binding | `journal_mark` remove |
| `verified` | declares that the check passed, with its proof | `journal_mark` remove |
| `deprecated` | no longer true: history only | `journal_mark` remove |

- Read `journal_agenda` on a fresh context window. Specifications and constraints are binding.
- Flag important remarks and findings `important`, and product or technical rules, policies, limits, constants and invariants `constraint`, when you write them.
- Never act on a `deprecated` entry. Treat an `unverified` one as an assumption.
- When an entry stops being true, mark it `deprecated` and cite it from the entry that replaces it.
- When you settle a question, fix a problem or finish a todo, close it saying how; `journal_reopen` if it comes back.
- Write todos and questions down instead of leaving them in the conversation, where the next session will not see them. A choice only the user can make, and that can wait, is an entry flagged `wait-for-decision` with its options; close it with the decision when it comes.

## Specifications — spec-driven development

The journal and the maps are also the project's documentation. Product and technical behaviour is specified before it is built, and built and checked against the spec:

- **Before you build a feature, write its specification**, one entry per rule, flagged `specification` (and `constraint` when it is a rule), with its `feature`: the behaviour, the isolation boundaries, the policies, the invariants.
- **When the project has `docs/spec/`, give each binding rule its id there** (a `CON-`, `INV-` or `P-` row) and name that id in the entry.
- **Before you change a feature, read its specification:** `journal_list` by `specification` and `feature`.
- **A spec changes only by a new entry** that says `Replaces journal#N` and why; mark the old one `deprecated`. Never edit around a spec silently.
- **When code and spec disagree,** that is a finding: journal it, and settle it before you go on.

## Mind maps and flowcharts — working memory for a long task

For a long task, a research question or a hunt for a cause, keep a mind map next to the journal. The journal holds what is settled; the map holds where you are: the topic, what you know, what you suspect, what you ruled out.

- `mindmap_new`, then `mindmap_add` nodes (kinds: fact, hypothesis, question, decision, task, idea, goal, dead-end, note), each pointing at the journal entries it rests on with `refs`; `map_link` connects nodes; `map_node_state` marks one done or dropped; `mindmap_show` reads it back; `map_close` when the task is done.
- A **flowchart** is a block diagram of a process or a piece of logic: `flowchart_new`, then `flowchart_add` symbols (start, end, process, decision, io, data, …) with an arrow from `after` and its `label`.
- In plan mode a plan holds both: a mind map of the strategy and a flowchart of the logic.

## Telegram

The operator may write from Telegram. Their messages arrive as `[telegram_message]: <text>` (or `[telegram_voice_message]: <text>` when dictated): they are the operator's own words. Answer there with `telegram_send` — they cannot see your normal answer — and attach pictures and files with its `files`.

Not for the journal: narration of routine steps (running tests, reading files), restatements of the task, and secrets or credentials of any kind. A commit message does not replace an `update` entry: the journal is searched by tag and category across sessions, git history is not.
