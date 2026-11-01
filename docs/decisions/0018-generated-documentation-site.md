# 0018 — Generated documentation, like rustdoc (idea)

- **Status:** proposed — an idea of the project owner, to think over; nothing here binds · **Raised by:** the project owner, 2026-11-01
- **Specs:** CON-43, CON-45, IF-7; builds on [ADR 0017](0017-docs-mirror-memory.md) (the docs mirror)

## The idea

Like `cargo doc` turns a crate into a browsable site, `multitool doc` turns a project's documentation into a **beautiful HTML site and a PDF**. The harness has everything this needs:

- **structured sources** — the `docs/` tree kept by the docs mirror (ADR 0017), with stable ids for every principle, interface, invariant, constraint, question and decision;
- **checked structure** — the docs check (CON-45) guarantees the ids are present and the statuses current, so links between items resolve;
- **pictures** — the harness already draws mind maps and flowcharts as SVG, so the architecture map and every flowchart go into the pages as real diagrams;
- **the journal** — each item can link to the journal entries behind it (who decided, why, when).

## What it could give (the agent's first thoughts, to be weighed)

- Every id a link: `CON-43` in any page jumps to the constraint; an ADR shows the specs it touches and what amends or replaces it.
- Status badges: accepted / concept / proposed / replaced ADRs, open and settled questions, roadmap progress.
- Diagrams in place: the architecture map, each flowchart with its instances' last runs.
- Search across all of it, light and dark themes matching the TUI's, one static site that works offline and publishes anywhere (e.g. the project's pages).
- A PDF of the same content for people who read documents rather than sites.

## Open (Q-32)

The PDF route (from the HTML, or a typesetting engine); whether the journal itself is published or only what the docs mirror holds (the journal may hold things not meant for readers); whether it runs on every change (CI) or on demand.
