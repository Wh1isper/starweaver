# Starweaver Memos

This directory holds active working notes, implementation evidence, reference comparisons, and release-preparation reminders. Long-lived architecture decisions live in `spec/`.

Current memos:

- `implementation-todo.md` — current audited roadmap, prioritized parity work, validation gates, and refactor sequence.
- `implementation-execution-plan-2026-06-07.md` — expanded implementation task graph and batch validation plan.
- `agent-sdk-foundation-plan.md` — landed Agent SDK P0/P1 foundation evidence and follow-up SDK deepening items.
- `builtin-tool-alignment-audit.md` — provider-native tool mapping, first-party bundle status, replay fixture gaps, and advanced tool follow-ups.
- `sdk-host-tool-gap-report.md` — executable web/search/scrape/download/media host tool status and remaining adapter depth.
- `pre-1.0-reference-notes.md` — compact pre-1.0 readiness checklist and reference map.
- `audit-evidence-2026-06-07.md` — requirement-by-requirement evidence map for the June 2026 audit.

Memo maintenance rules:

- Keep completed history brief and move durable decisions into `spec/`.
- Mark each major area as `landed`, `partial`, `pending`, or `postponed`.
- Record evidence with file paths and validation commands.
- Remove stale phase plans after the implementation lands.
