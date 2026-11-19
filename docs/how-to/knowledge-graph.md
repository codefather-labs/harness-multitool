# Keep the knowledge graph

A feature as a work branch:
1. a todo with its test protocol;
2. a mind map linked to it;
3. the change, written as an update flagged unverified;
4. the proof, written as a verification flagged verified;
5. the close, which would refuse without that proof.

A later question is answered from the journal, not from memory.

<!-- how-to: knowledge-graph -->

On [harness-multitool.dev](https://harness-multitool.dev/docs/how-to/knowledge-graph) this page plays it live (CON-187).

## In the real harness

- The agent writes with `journal_add` and reads with `journal_search`, `journal_get`, `journal_list` and `journal_agenda`.
- `journal_close` closes a todo only once its branch holds a verified entry.
- `mindmap_new`, `mindmap_add`, `flowchart_new` and `flowchart_add` draw maps whose nodes point at entries. `/journal` and `/map` work from the composer.

## See also

[Journal](../journal.md), [Maps](../maps.md), [Agent workflow](../agent-workflow.md).
