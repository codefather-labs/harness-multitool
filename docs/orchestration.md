# Orchestration

In orchestration mode an agent becomes the master of agents it starts: it gives each a journal entry as its work, follows it, checks the result against the specs and closes the entry. Each agent it starts is a slave whose only contact is its master.

## What it is

Orchestration is a mode of the harness, not a utility ([Modes](modes.md#what-it-is)). The agent enters it with `mode__orchestrate` on. Its prompt then gains the mode's rules, and the tools to start, read, answer and close agents appear (CON-100).

Master and slave are technical terms only. The master is an agent like any other; its slaves are agents in tabs of its group, in the same project folder and git, with the journal and every tool.

## How it works

### Starting an agent

The master writes the work first, as an open journal entry, then starts an agent on it with `mode__agent_spawn` (CON-109):

| Argument | Is |
|---|---|
| `nick` | what the agent goes by; unique among the open conversations |
| `adapter`, `model` | one of those `mode__agent_models` lists as authorized |
| `task` | the id of the open journal entry that is its work |

A spawn without an open entry is refused. The agent starts in a new tab of the master's group, with no history, and its system prompt names its task. It starts on it at once.

A slave starts on its master's profile, and `mode__agent_models` lists what is signed in under it (CON-124). A hook tells each agent it works for the master and asks it to report ready.

### Who talks to whom

- A slave messages only its master, with `agent_send`.
- The master messages any of its slaves with `agent_send`. Its message arrives as the user's, so the master stands for the user there.
- A slave's `ask_user`, its plan and its permission requests go to its master, not to the user, by the user's own decision (CON-100).
- Messages from slaves, and word that a run ended, reach the master as data. The master ends its turn while they work: it is woken.

### The task is a journal entry

Only the master closes a slave's task (CON-109). The cycle, as [the agent workflow](agent-workflow.md#6-in-an-orchestra) holds it:

1. The slave reads the entry and works on it (W-50).
2. Its work done and proved, it asks its master with `agent_send` to close the entry, naming the proofs (W-57). If it calls `journal_close` or `journal_abandon` on its own task, the harness refuses it.
3. The master checks the work with `mode__agent_read` (W-54, W-59).
4. Good: the master calls `journal_close` and tells the slave. Not good: it sends the slave back (W-62).
5. On approval, the slave asks to close its session (W-58), and the master calls `mode__agent_close`.

The master can close an agent at any time.

### Checking against the specs

Guaranteeing that its agents' work matches the specs is the master's duty, since a slave may not keep track of them (CON-112).

Before approving, the master reads what binds the change (`journal_search`, `journal_list`) and reads the change itself against it, not only the slave's report. If the slave broke a spec, the master does not close the entry and does not fix it itself: it sends the slave back with `agent_send`, naming the rule and what breaks it. A spec that has to change takes the user's word.

### Plans and permissions

A slave plans like any agent. Its plan holds a mind map of its strategy pointing at its task, and a flowchart of the logic it builds; `mode__plan_exit` refuses it until both exist (CON-111).

The slave sends its plan to its master and waits. When the master says to go, the slave calls `mode__plan_exit` with the master's words in `user_said` (W-56). A slave's plan is never put to the user's window or Telegram (CON-163).

Without asking anyone, a slave reads and writes files inside its folder, runs reading commands, and builds and tests with `cargo` (CON-103). Anything else goes to its master: other commands, redirection, substitution, the network, a path outside the folder.

On an agent backend a slave always runs isolated: none of the user's settings, hooks, rules or servers, and never with permissions skipped (CON-108). Each request beyond the basics goes to the master with the tool and its arguments, through `mode__permission_ask`, and the program waits.

The master answers with `mode__agent_answer`: `once`, `always` (for this tool and target in that session) or `deny`. No answer in 30 minutes, or no master, is a deny.

### Reminders

While a slave lives, the harness reminds its master every 10 minutes to check on it, naming the agent, its task and whether it is running (CON-110). On a reminder the master calls `mode__agent_read`; a slave that stopped with no word is told to move on, or closed if its work is over (W-60).

### In the window

The group's tabs are set off together, with (M) and (S) on their titles. The status line's MODE says `orchestrate` on the master and `slave` on its agents ([Modes](modes.md#the-status-line)).

## Using it

| Tool | For |
|---|---|
| `mode__orchestrate` | the mode on or off (`on`) |
| `mode__agent_models` | the adapters an agent can start on, with their models |
| `mode__agent_spawn` | start an agent on a task: `nick`, `adapter`, `model`, `task` |
| `mode__agent_read` | what an agent has done: each message it got, each call in order with its arguments, its last words (`nick`) |
| `mode__agent_answer` | answer an agent's permission request: `nick`, `answer` |
| `mode__agent_close` | end an agent: its process stops and its tab closes; what it wrote stays (`nick`) |
| `agent_send` | give an agent work, or tell it a verdict |

The `mode__agent_*` tools work only in orchestration mode. On an agent backend they come over the `mode` server as `mcp__mode__agent_spawn` and so on; its list is fixed at the start, so outside the mode they are refused rather than absent ([Tools and modes](tools.md#modes)).

`mode__orchestrate` with `on` false ends the mode. The master's agents stay until it closes them.

### The workflow rows

[The agent workflow](agent-workflow.md#6-in-an-orchestra) binds both sides:

| Row | Who | Situation |
|---|---|---|
| W-50 | slave | just started on a journal entry: read the file and the entry, work on it |
| W-51 | slave | a user message in its tab stands for the user |
| W-52 | slave | done or in need: `agent_send` its orchestrator, its only contact |
| W-55 | slave | a question: `ask_user`, which goes to the orchestrator |
| W-56 | slave | plan ready: send it, wait, then `mode__plan_exit` with the master's words |
| W-57 | slave | work done and proved: ask to close the entry; do not close it |
| W-58 | slave | approved: ask to close its session, and stop |
| W-53 | master | work for an agent: write the entry, spawn with it as `task`, end the turn |
| W-54 | master | checking work: `mode__agent_read`, against the rows of the workflow |
| W-59 | master | asked to close an entry: check, then `journal_close` or send back; then `mode__agent_close` |
| W-60 | master | a reminder: `mode__agent_read`, then move it on or close it |
| W-62 | master | checking a change: read what binds it and the diff; a broken spec goes back named |

## Limits and status

- At most 8 agents per master. A slave cannot start agents of its own (CON-100).
- A permission request unanswered for 30 minutes is denied (CON-108).
- A brainstorming mode, with agents as peers rather than master and slaves, is planned in the [roadmap](spec/roadmap.md) and not built.

## Specs and decisions

- CON-100: orchestration mode, master and slaves
- CON-103: what a slave does without asking
- CON-108: a slave's permissions on an agent backend
- CON-109: an agent is bound to a journal entry
- CON-110: reminders every 10 minutes
- CON-111: a slave's plan
- CON-112: the master checks against the specs
- CON-124: a slave's profile
- CON-163: a slave's plan stays its master's
- [Agent workflow](agent-workflow.md#6-in-an-orchestra): W-50 to W-62
- [Agents that talk to each other](agents.md)
