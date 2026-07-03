# Python Stability And Known Gaps

This page lists the boundaries that should stay explicit while the Python SDK
is still young. Treat them as refactor candidates, not as product polish.

## Stable Contracts

- Python tools are injected in-process as native Starweaver runtime tools.
- Rust remains the source of truth for model request preparation, tool
  scheduling, retries, usage, trace, and session state.
- `StreamEvent.raw` is the portable canonical stream record.
- `RunResult.raw_state` and `SessionArchive` carry raw Starweaver state for
  durability boundaries.
- Tool calls run in parallel by default unless the tool is marked
  `sequential=True` or duplicate same-name calls require ordered execution.

## Current Facades

`run_stream()` currently returns `AgentRun`, which is both an async iterator over
canonical stream records and a facade for `recv()`, `join()`, `result()`,
`status()`, `recoverable_state()`, `interrupt()`, active messages, steering, and
streamed HITL helpers. The canonical records are the stable evidence surface.
The Python live-control shape should remain easy to revise until the Rust
live-stream and interrupt contract is intentionally frozen for Python.

`AgentStream` is a compatibility alias for `AgentRun`; new examples use
`AgentRun`.

## Known Gaps

- Python provider model IDs do not resolve CLI gateway profiles such as
  `homelab@openai-responses-ws:gpt-5.5`. Use Python provider IDs, direct
  `base_url`/`api_key_env` overrides, or `oauth@codex:` until CLI profile
  resolution is exposed.
- Streamed HITL `resume_collected(...)` is collected resume through the owning
  session, not a live continuation handle. `run.join().events` remains the
  original suspended stream records and does not include post-resume records.
- `SessionArchive` serializes Starweaver state, not Python callables, provider
  connections, media upload callbacks, or live environment handles.
- `EnvironmentProvider` handles are process-local and must be reattached after
  restore.
- `SkillRegistry` loads native Starweaver skill packages; there is no separate
  Python skill authoring DSL.
- `CapabilityBundle` is static composition. Python hook-level capabilities need
  a typed Python hook contract before becoming public API.
- `MediaUploader` adapts a callback into the native filter, but the host still
  owns resource lifecycle, storage, and access control.
- `StreamAdapter` projects collected or replayed records only. It is not a
  stream owner and cannot interrupt, steer, resume, or continue a run.

## UX Rules For New Python APIs

- Prefer explicit scope over global mutation: agent defaults, per-run overrides,
  and session state should stay visibly separate.
- Prefer canonical Rust records plus typed Python helpers over Python-only
  hidden state.
- Do not serialize process-local Python objects into durable session archives.
- Do not make subprocess or MCP-style bridges the default for Python tools.
- Do not document live-control ergonomics as stable until the underlying Rust
  contract is intentionally finalized for Python.
