# Project journal

The journal is a project's record of why it is the way it is: its decisions, findings and work items, kept across sessions and agents. It is also the project's changelog, and the plan an agent works from.

## What it is

Each project has one journal (CON-22). A project is its repository remote, or its folder when it has none, so every checkout of a repository shares the same journal.

The harness's daemon keeps the journals, in the OS's data directory under `journals/` (CON-20). A folder that is no git repository keeps its journal at `~/.harness/projects/<name>-<hex>/journal.db` instead (MULTITOOL_HOME when set; CON-197); a harness run on a data dir of its own, a test's, keeps those under that data dir's `home/`. An agent reaches its project's journal only through the journal tools; a journal tool works on that project's journal only. The browser build implements the same interface over browser storage.

An entry is a short note, up to 20,000 characters, with:

- a **category**: what kind of entry it is, such as `decision`, `finding`, `bug`, `lesson`, `update`, `verification`;
- **tags**: the words it is found by again;
- **flags**: what is open about it and how far to trust it (below);
- a **salience**: how long it is kept;
- optionally a **feature**, what **blocks** it, and its **proof**.

## How it works

### Flags

An entry carries any number of flags (CON-21). Some are work items that stay open until closed; the others are marks.

| Flag | Meaning |
|---|---|
| `todo` | something to do |
| `question` | something to find out |
| `wait-for-decision` | a choice only its decider, usually the user, makes; the work that depends on it waits (CON-178) |
| `problem` | an unresolved problem |
| `unverified` | a hypothesis or an implementation not yet checked |
| `specification`, `constraint` | how something must behave, a rule or a limit; binding |
| `invariant`, `principle` | binding, mirrored to `docs/spec/invariants.md` and `docs/spec/principles.md` with their doc id (CON-115) |
| `research` | the result of a research or an analysis, with its sources; not open work (CON-179) |
| `important`, `idea`, `concept`, `comment` | keep in view, or thinking that does not bind |
| `verified` | the check passed, with its proof |
| `deprecated` | no longer true: history only |

CON-21 also lists `bugreport`, `reminder` and `disputed` among the work items.

### Salience

Salience is retention. `high` is kept for good, `medium` (the default) for a year, `low` for 90 days. An entry with an open work item or an `important` mark stays until that is closed or removed.

### Links

Every `journal#N` in an entry's text links the two entries, and both show the link. The verb just before it sets the kind:

| Written | Link |
|---|---|
| "Implements journal#N" | implements |
| "Verifies journal#N" | verifies |
| "Fixes journal#N" | fixes |
| "Replaces journal#N" (also supersedes, corrects, reverses) | replaces |
| anything else | cites |

`journal_link` links two entries whose text does not mention each other, as `cites`, `replaces` or `fixes`. A link never changes a flag: an entry that is replaced is marked `deprecated` by hand.

### Branches

A feature or a fix runs as a branch, from a todo to its evidence:

1. a `todo` with its test protocol: what to test, the regressions, how, under which conditions, what evidence proves it;
2. an `update` flagged `unverified`: "Implements journal#T", with how to verify it;
3. a `verification` flagged `verified`: "Verifies journal#I", with its `proof`;
4. `journal_close` on the todo.

Only Implements, Verifies and Fixes links make an entry part of a branch; it belongs to its nearest todo. A bug is a branch too: a `bug` flagged `todo`, then "Fixes journal#B", then its verification.

### Evidence first

A journal is evidence-first unless a project turns it off (CON-101). A verification must point at a proof: in words with `proof` (a test's name, a command and its result, a commit, a URL), or as a file with `proof_file`.

A proof file is kept in the project's `.harness/proofs/`. A todo closes only on a verified entry in its branch, and only when nothing blocks it.

### Blocking

`blocked_by` says an entry cannot close until another has. Closing the blocker answers with `released`: the entries it set free. A blocker that is abandoned leaves its waiters under `blocker_dropped` in the agenda, to be re-read.

### The graph

`journal_search` and `journal_get` answer each entry with its graph: the entries linked to it, each as its id, a title of at most 80 characters and the link that reaches it (CON-118).

They follow links out to `depth` hops, 2 by default, nearest first. An answer holds at most `search_max_linked` linked entries (100 by default, 0 for no paging); `offset` reads the next page. Search is lexical: there is no model and no search by meaning (CON-119).

Mind maps and flowcharts join the same graph: their nodes point at the entries they rest on ([Maps](maps.md#refs-to-the-journal)).

## Using it

| Tool | For |
|---|---|
| `journal_add` | one decision, finding or update, with the why; `blocked_by`, `proof`, `proof_file` as needed |
| `journal_search` | earlier entries by words, with their linked entries |
| `journal_get` | one entry in full, with its flags, blockers and links |
| `journal_list` | entries newest first, by category, tags, flags, feature, open or closed |
| `journal_agenda` | what is open, on a fresh context: work that can be taken now, `blocked`, `blocker_dropped` |
| `journal_tags` | the tag vocabulary and categories, most used first |
| `journal_mark` | set or remove flags, or set or lift a block |
| `journal_close` | close an entry's open work items, saying how |
| `journal_abandon` | close a todo whose work was dropped, saying why |
| `journal_reopen` | reopen closed work items |
| `journal_branch` | an entry's branch: its todo, implementation, evidence, and how it stands |
| `journal_link` | link two entries whose text does not mention each other |

Each tool is described in [Tools and modes](tools.md#the-journal). On an agent backend such as Claude Code they are `mcp__harness__journal_add` and so on.

When and how an agent writes to the journal is set by [the agent workflow](agent-workflow.md#2-every-session): rows W-10 to W-18 for every session, W-20 to W-24 for specs, W-40 to W-47 for building and proving.

### In the terminal UI

Each journal tool's result is a JOURNAL card in the transcript. Writes to memory need no permission prompt.

The system prompt carries the project's specifications and constraints from the journal, and the titles of its maps. In a project with `docs/`, the agent is told which file to update after each write, and `multitool docs check` compares the docs with the journal ([the TUI](tui.md#setting-up-a-project)).

A `#N` ref on a mind map or flowchart opens the entry as a ticket beside the map ([Tickets](maps.md#tickets)).

## Settings

**Settings > Journal** sends the window's project's todos to its GitHub issues, through the daemon ([ADR 0028](decisions/0028-journal-todos-as-tracker-issues.md)):

- a todo opens an issue, titled by its first line;
- an entry linked to it comments on that issue;
- closing the todo closes the issue; abandoning closes it as not planned; reopening reopens it.

It is per project and off until turned on (CON-139, CON-142). Three things are needed: a GitHub account connected through gh on the tab, the option on, and a GitHub remote. When one is missing, the tab says which.

gh is connected only by its own `gh auth login`. The harness never reads or stores a GitHub token, never runs `gh auth switch`, and names `GH_CONFIG_DIR` on every call (CON-141). On a public repository, turning the option on asks once.

A journal write never waits on GitHub: the daemon queues the work and retries it, and keeps a todo-to-issue map so nothing is sent twice. `multitool journal issues` does the same from the command line.

The tab lists GitLab, Asana and ClickUp as trackers that are not available yet: they exist behind the same interface with empty bodies (CON-143).

## Limits and status

- One journal per project; every checkout of a repository shares it.
- Search is lexical only (CON-119).
- An entry is at most 20,000 characters.
- Only GitHub is implemented as an issue tracker.

## Specs and decisions

- CON-20, CON-21, CON-22: the journal, its flags, one per project
- CON-101: evidence first
- CON-115: `invariant` and `principle` flags
- CON-118, CON-119: the graph in answers, lexical search
- CON-178, CON-179: `wait-for-decision` and `research`
- CON-139 to CON-143: journal todos as GitHub issues
- [ADR 0017](decisions/0017-docs-mirror-memory.md): the docs mirror the journal
- [ADR 0028](decisions/0028-journal-todos-as-tracker-issues.md): journal todos as tracker issues
- [Agent workflow](agent-workflow.md): W-10 to W-47
