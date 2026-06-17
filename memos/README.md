# Starweaver Memos

This directory is intentionally small. Long-lived architecture decisions live in `spec/`; user-facing guidance lives in `docs/`.

Current memos:

- `implementation-todo.md` — ready-to-go checklist, validation gate, and small post-release parking lot.
- `provider-sdk-alignment-research.md` — provider SDK alignment research, recommended validation path, and implementation direction.
- `provider-parameter-gap-and-rollout-plan.md` — provider parameter alignment baseline, implemented status, final validation evidence, and remaining typed-provider-setting follow-ups.
- `context-cache-investigation.md` — prompt-cache investigation, implemented routing hardening, and validation evidence.
- `model-settings-session-affinity-status.md` — implemented session-affinity routing status for typed `ModelSettings`, `AgentContext.session_id`, TUI affinity, Gateway sticky routing, and Codex OAuth headers.
- `trace-implementation-review.md` — trace implementation review and remaining observability follow-ups.

Memo maintenance rules:

- Keep only active readiness notes or release-preparation reminders.
- Move durable decisions into `spec/` before deleting detailed working notes.
- Remove stale audit notes once their findings are implemented and covered by tests.
