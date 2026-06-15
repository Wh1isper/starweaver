# Starweaver Memos

This directory is intentionally small. Long-lived architecture decisions live in `spec/`; user-facing guidance lives in `docs/`.

Current memos:

- `implementation-todo.md` — ready-to-go checklist, validation gate, and small post-release parking lot.
- `hitl-audit-2026-06-08.md` — HITL audit findings, implementation evidence, and validation commands.
- `cache-investigation-2026-06-12.md` — CLI/provider cache investigation, Pydantic AI alignment gaps, live API evidence, and fix roadmap.
- `model-provider-alignment-audit-2026-06-12.md` — full Starweaver model/provider audit against latest Pydantic AI, with cache, reasoning replay, server-state, streaming, and test gaps.
- `agent-context-ya-mono-parity-report.md` — field-by-field AgentContext parity audit against ya-mono, covering context fields, resumable state, bus/tasks/notes, subagents, and migration recommendations.

Memo maintenance rules:

- Keep only active readiness notes or release-preparation reminders.
- Move durable decisions into `spec/` before deleting detailed working notes.
- Remove stale audit notes once their findings are implemented and covered by tests.
