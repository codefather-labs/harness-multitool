# 0028 — Journal todos become issues in a tracker

- **Status:** proposed · **Asked by:** the project owner, 2026-11-11 (journal#939). The agent drafted the design overnight; the owner's questions are open below.
- **Specs:** CON-139, CON-140, CON-141, CON-142, CON-143

## Why

The journal is the record of the work, but the owner also wants the work where a team sees it: the project's tracker, GitHub first. A todo should open an issue. What is linked to the todo should comment on it. Closing the todo should close the issue. Other trackers (GitLab, Asana, ClickUp) come later, behind the same interface.

## Decision (draft)

### Where it runs

The daemon owns the journals, so the daemon sends. No window sends, and no agent does. A journal write never waits on a tracker:
- each change that concerns a tracker goes into a queue, kept beside the project's journal (`<data dir>/journal-trackers/<project>/queue.jsonl`);
- a worker sends the queue in order;
- it retries with backoff (2 s, doubling, at most 5 min) and keeps the queue across restarts.

### The interface

```rust
pub trait IssueTracker: Send + Sync {
    fn name(&self) -> &str;                                   // "github", "gitlab", "asana", "clickup"
    fn connected(&self, project: &ProjectPlace) -> Result<Account, TrackerError>; // as whom
    fn can_take_issues(&self, place: &ProjectPlace) -> Result<Visibility, TrackerError>; // and public or private
    fn open(&self, place: &ProjectPlace, title: &str, body: &str) -> Result<IssueRef, TrackerError>;
    fn comment(&self, place: &ProjectPlace, issue: &IssueRef, body: &str) -> Result<(), TrackerError>;
    fn close(&self, place: &ProjectPlace, issue: &IssueRef, how: Closing, note: &str) -> Result<(), TrackerError>; // Done | NotPlanned
    fn reopen(&self, place: &ProjectPlace, issue: &IssueRef) -> Result<(), TrackerError>;
}
```

`ProjectPlace` is the tracker's own address of the project:
- GitHub: `owner/repo`, from `git remote get-url origin`;
- GitLab: `group/project`;
- Asana: a project's gid;
- ClickUp: a list's id.

GitHub is implemented through the gh CLI. GitLab, Asana and ClickUp are empty bodies that answer `TrackerError::NotImplemented`, and the Journal tab lists them as not available (CON-143).

### GitHub through gh

The rules of CON-141 hold. The harness:
- runs the unmodified gh;
- passes `GH_CONFIG_DIR=<data dir>/journal-trackers/<project>/gh` explicitly on every call;
- never runs `gh auth switch`;
- never reads a token.

Connecting is `gh auth login` in that folder, run in the terminal like Claude Code's login. The calls:
- `gh issue create --repo owner/repo --title … --body-file -`, the body on stdin;
- `gh issue comment N --repo … --body-file -`;
- `gh issue close N --repo … --reason completed|"not planned" --comment …`;
- `gh issue reopen N --repo …`;
- `gh api user`, for the login;
- `gh repo view owner/repo --json visibility,viewerPermission`.

### What goes

| Journal | Tracker |
|---|---|
| a todo, a bug flagged todo, or a bugreport added | an issue: the title is the first line (cut to 120 characters), the body is the entry, its flags, tags and `journal#N` |
| an entry linked to it (Implements, Verifies or Fixes in its branch; one that cites it) | a comment: category, flags, `journal#N`, the body |
| the todo closed | the issue closed (`completed`), the resolution as its last comment |
| the todo abandoned | the issue closed (`not planned`) |
| the todo reopened | the issue reopened |

The map from todo to issue lives in `<data dir>/journal-trackers/<project>/issues.json`. A queue item carries the entry id and the action, so a replay after a restart sends nothing twice: the map is checked first.

### Settings > Journal (per project, CON-142)

The tab shows:
- the project's name;
- the tracker, a choice of GitHub, GitLab, Asana or ClickUp, with the last three marked not available;
- the connection: the login, with Connect and Disconnect;
- the place: owner/repo from the remote, and whether it is public or private;
- the option 'Send journal todos to issues', off by default. On a public repository, turning it on asks once.
- When the option is on but something is missing, the tab says what: not connected, no remote, or no right to write issues.

The settings are the daemon's, in `<data dir>/journal-trackers/<project>/settings.json`, never in the repository.

## Open questions for the owner (journal#939)

1. 'В дашборду проекта': only the repository's issues, or also cards on a GitHub Projects board? If a board, which one, and in which column?
2. When the option is turned on, are the open todos that exist already sent? The draft sends the open ones and none of the closed.
3. What counts as linked? The draft takes the branch (Implements, Verifies, Fixes) and the entries that cite the todo.
4. Labels: should issues carry `journal` and the entry's category? The draft adds the label `journal` if it exists, and creates no labels.

## Rejected

- **A window sends.** Windows come and go, and two windows would send twice.
- **The GitHub API with a token the harness keeps.** That would break CON-141 and the owner's two-account setup, where gh is per config directory.
- **One tracker hard-wired.** The owner asked for an interface (CON-143).
