# Project Journal

Every project has a journal: the decisions, findings and updates of every earlier session and agent. It is two things at once — the record of **why** the project is the way it is (decisions and findings, so the next session does not re-decide or re-discover them) and the project's **changelog** (updates: what changed, when, where). Read it before you decide, write to it as you work.

This file says what the journal is and how to write it. **What to do, and when** — plan mode, the work branch from todo to evidence, spec first, the maps — is the agent workflow specification, `~/.local/share/harness-multitool/spec/agent-workflow.md`, which the harness hands you at a session's start. It is the one source of the procedure: where this file and it seem to differ, it wins. The journal is the one memory of a project: keep no notes of it elsewhere.

## Where it lives and whose it is

- The multitool harness's daemon owns every journal, one per project. You reach it only through the harness's tools (`journal_*`, `mindmap_*`, `flowchart_*`, `map_*`; on an agent backend they are `mcp__harness__journal_add` and so on). Never open, copy or edit the journal files yourself.
- The project is the repository of the folder the session runs in (its git remote), or that folder itself when it has no remote — so every checkout of a repository shares one journal. A folder that is no repository keeps its journal in `~/.harness/projects/` (CON-197). Every tool works on that project's journal only.
- If a journal tool answers that the daemon is not reachable, tell the operator once and carry on without journaling; do not work around it.

## Read — before you decide

On a fresh context window (session start, resume, after compaction), and before any non-trivial decision:

- `journal_agenda` — what is open: todos, questions, choices waiting for the user's decision (`wait-for-decision`), problems, and the specifications and constraints that bind the work;
- `journal_tags` — the tag vocabulary, most used first;
- `journal_search` — by words, ranked; its `linked` names the neighbours of what it found;
- `journal_list` — by category, tags, flags, feature, open or closed, newest first;
- `journal_get` — one entry in full, with its links.

Read by category and tag, not by paging through everything. Before a research or an analysis, look for an earlier one (`journal_list` with flags `research`, `journal_search`) and do not redo what still holds.

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
- **research** — not a category but a flag: the result of any research or analysis (a cause hunted, options compared, a system or a codebase read, a measurement), flagged `research` on its entry (a finding, a decision…). It stands alone: what was asked; how it was looked into (method, commands, data, conditions); its sources named exactly (`path:line`, a URL, a version, a command and its output); the result, with numbers where there are any; how sure it is and what it does not cover. Give it `salience` high when it should be kept for good.
- **update** — a change you made to the project: code, config, docs, dependencies, infrastructure. What changed and where (files, the commit hash once there is one), and why — cite the `journal#<id>` of the decision behind it when there is one. These entries are the changelog.

Reuse the categories and tags the journal already has before inventing new ones.

How to write one:

- **Self-contained.** A later session reads it without this conversation: name the component, the file (`path:line`), the command, the version.
- **The why, not only the what.** "Chose WAL mode because concurrent readers blocked the daemon's writer (journal.rs:open); rejected a write queue as more moving parts" — not "switched to WAL".
- **Tags are how it is found again.** 1–4 specific nouns — component, technology, feature (`sqlite`, `daemon`, `telegram`, `install`); reuse existing tags rather than near-synonyms.
- **Salience is retention.** `high` — forever, for load-bearing decisions and constraints; `medium` (default) — a year; `low` — 90 days, for ambient context. Do not mark everything `high`.
- **Reversals cite what they reverse.** "Replaces journal#42: …".

## Work branches

A feature or a fix runs as a branch of the journal: a `todo` with its test protocol, then `Implements journal#T` flagged `unverified`, then `Verifies journal#I` flagged `verified` with its `proof` or `proof_file` (under the project's `.harness/proofs/`), then `journal_close` of the todo, which refuses until the branch holds the evidence. A bug is the same branch: `bug` flagged `todo`, then `Fixes journal#B`, then the verification. Dropped work closes with `journal_abandon`; blocked work sets `blocked_by`. The steps and their order are the spec's W-32, W-40 to W-47. `journal_branch` shows a branch from any of its entries. Only `Implements` / `Verifies` / `Fixes` make an entry part of a branch, and it belongs to its nearest todo; other mentions cite.

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
| `research` | the result of a research or an analysis: what was asked, how, its sources named exactly, the result, how sure it is | `journal_mark` remove |
| `verified` | declares that the check passed, with its proof | `journal_mark` remove |
| `deprecated` | no longer true: history only | `journal_mark` remove |

- Read `journal_agenda` on a fresh context window. Specifications and constraints are binding.
- Flag important remarks and findings `important`, and product or technical rules, policies, limits, constants and invariants `constraint`, when you write them.
- Never act on a `deprecated` entry. Treat an `unverified` one as an assumption.
- When an entry stops being true, mark it `deprecated` and cite it from the entry that replaces it.
- When you settle a question, fix a problem or finish a todo, close it saying how; `journal_reopen` if it comes back.
- Write todos and questions down instead of leaving them in the conversation, where the next session will not see them. A choice only the user can make, and that can wait, is an entry flagged `wait-for-decision` with its options; close it with the decision when it comes.

## Specifications and maps

A binding rule is an entry flagged `specification` (and `constraint` when it is a rule), with its `feature`; where the project has `docs/spec/`, it names its `CON-`/`INV-`/`P-` id there. A spec changes only by a new entry that says `Replaces journal#N`, the old one marked `deprecated`. When code and spec disagree, that is a finding to settle. The spec's W-20 to W-24.

A mind map holds where you are in a long task (`mindmap_new`, `mindmap_add` with node kinds fact, hypothesis, question, decision, task, idea, goal, dead-end, note, each with `refs` to the entries it rests on; `map_link`, `map_node_state`, `mindmap_show`); a flowchart is a process or a piece of logic (`flowchart_new`, `flowchart_add` with `after` and `label`). A plan holds both. Maps, flowcharts and entries form one knowledge graph that outlives each plan: a change brings up to date what of it the change touches, and links a new map to the maps it crosses (`map_link` with `to_map`). The spec's W-33, W-34 and W-37 (CON-196).

## Telegram

The operator may write from Telegram. Their messages arrive as `[telegram_message]: <text>` (or `[telegram_voice_message]: <text>` when dictated): they are the operator's own words. Answer there with `telegram_send` — they cannot see your normal answer — and attach pictures and files with its `files`.

Not for the journal: narration of routine steps (running tests, reading files), restatements of the task, and secrets or credentials of any kind. A commit message does not replace an `update` entry: the journal is searched by tag and category across sessions, git history is not.
