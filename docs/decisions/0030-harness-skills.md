# 0030 — Skills of the harness's own, the same on every adapter

- **Status:** accepted (journal#971; built in journal#987) · **Asked by:** the project owner, 2026-11-11 (journal#888; and 'скилы стоит сделать свои для переносимости между адаптерами', journal#905). The agent drafted the design overnight.
- **Specs:** CON-135, CON-145 (the delegates' own plugins and skills stay theirs)

## Why

A skill written once should work whatever runs the conversation: the harness's own loop, Claude Code or Codex. Claude Code and Codex each have skills of their own, and those keep working inside them (CON-145). The harness's skills are the portable layer above them.

## Decision (draft)

### Where skills live

They are laid out as Claude Code lays out its own, with `.harness` in place of `.claude`:

```
<project>/.harness/skills/<folder>/SKILL.md     the project's
~/.harness/skills/<folder>/SKILL.md             the user's, global (MULTITOOL_HOME when set)
```

`SKILL.md` starts with YAML frontmatter with `name` and `description`, and the body is the instructions. Files beside it (scripts, templates) are the skill's own, read by path.

- **External contract, verify before building:** Anthropic's Agent Skills format.
  - Frontmatter fields `name` (lowercase, digits, hyphens) and `description` are required; `allowed-tools` is optional.
  - Verify: docs.claude.com, Agent Skills; record it in an `### External contracts` entry with the doc's URL.
- The same names on both levels: the project's wins (journal#888).
- A skill folder of Claude Code's (`.claude/skills`) is not read by the harness: it is Claude Code's own, and stays so (CON-145).

### How an agent uses them

Two tools are served by the harness's MCP server to every adapter. The native loop gets them as tools; Claude Code and Codex get them through the `harness` server they are already given.

- `skill_list`: the skills here, each with its name, description, and whether it is the project's or the user's. Only descriptions, so a long list costs little.
- `skill_load(name)`: the skill's body, and the paths of its files.

The specification of the agent workflow gains a row: when a task matches a skill's description, `skill_load` it first.

### On the screen

- `/skills` lists them.
- A skill loaded in a turn shows as a card, `SKILL name`, folded.

## Creating skills, and the delegates' copies (CON-204)

The owner's word 2026-11-27 (journal#1705). An agent makes a skill with `skill_create {name, description, body, scope, files}`, changes it with `skill_edit` and removes it with `skill_delete`, on every adapter.

- **The source is the harness's.** It lives in `<project>/.harness/skills/<name>/` for scope `project`, or `~/.harness/skills/<name>/` (MULTITOOL_HOME's) for `user`, in the format above.
- **Each delegate gets a native copy by default.** Claude Code's goes in `<project>/.claude/skills/<name>/` or the user's own Claude Code folder's `skills/<name>/`. Codex's goes in `<project>/.agents/skills/<name>/` or `$CODEX_HOME/skills/<name>/` (`~/.codex/skills`). So each program finds the skill as its own, and Claude Code picks it up within its running session.
- **A copy carries a marker**, `.multitool-skill`, with its source and sha256. Edit and delete touch only marked copies. A native skill of the same name with no marker is the user's: it is left alone, and the answer says so.
- **Names and descriptions follow Anthropic's Agent Skills rules.** A name has at most 64 lowercase letters, digits and hyphens, without `anthropic` or `claude`. A description is non-empty, at most 1,024 characters, and has no XML tags.
- **Every file written is asked for**, the delegates' too, as a WritePath.

## Open questions for the owner

1. Should the harness also offer the delegates' own skills (`~/.claude/skills`) to the native loop and Codex, read-only? The draft says no; each program keeps its own, and only `.harness/skills` is portable.
2. Should a skill be able to declare which adapters it is for? The draft says no, a skill is for all.

## Rejected

- **Copying `.harness/skills` into Claude Code's and Codex's skill folders.** That writes into the user's files (CON-145), and the two formats may part ways.
- **A plugin system of the harness's own.** Dropped by the owner (CON-135, journal#905).
