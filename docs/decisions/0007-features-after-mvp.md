# 0007 — What comes after the MVP

- **Status:** accepted · **Decided by:** the project owner, 2026-10-31
- **Specs:** P-3, P-4, P-5, P-9, INV-4, INV-5, IF-3, IF-8, IF-9, IF-10, IF-12

## Decision

After the MVP, four groups of features, the ones that set the harness apart:

1. **Done = proven + specification.** The verification gate is part of the loop (INV-4); specifications become acceptance checks, and the specifications that bind the files in play are put into the context automatically.
2. **Memory instead of summary.** Context is rebuilt from the journal and the task's mind map instead of being summarised away; the loop keeps a live mind map of the agent's reasoning — goal, hypotheses, discarded paths, evidence — visible while it works.
3. **Rewind and branches.** Any session can be rewound to event N and branched from there (INV-5); several approaches run in parallel in separate worktrees and are judged by their tests and metrics; the losers stay in memory as discarded paths with the reason.
4. **Agent mesh and routing.** A task is split across agents on different machines, each with its own model; the router picks models per step within budgets.
