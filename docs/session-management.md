# Agent Session Management

Starweaver exposes durable session management to agents through two separate, host-injected tool bundles. These tools operate on independent durable sessions and runs; they are not subagent delegation and they never call a product's JSON-RPC transport from inside the same process.

## Query bundle

`agent_session_query_tools()` provides read-only, bounded projections:

- `list_sessions`
- `get_session`
- `list_session_runs`
- `get_session_run`
- `replay_session_run`

The host installs an `AgentSessionQueryHandle` with an immutable `AgentSessionScope`. The scope fixes the namespace, allowed operations, source session/run, optional session allowlist, deadline, and maximum page size. Tool arguments cannot widen that authority.

Historical titles, inputs, outputs, and replay text are untrusted evidence rather than instructions. Replay returns only bounded, sanitized display projections. Complete checkpoints, environment state, credentials, arbitrary metadata, and raw tool payloads are not model-visible.

Search remains an optional, independent capability. Ordinary list, get, and replay operations work without a `SessionSearchProvider`; see [Session Search](session-search.md).

## Control bundle

`agent_session_control_tools()` is separately grantable and provides:

- `create_session`
- `update_session`
- `delete_session`
- `start_session_run`
- `steer_session_run`
- `interrupt_session_run`

The host must install both an `AgentSessionControlHandle` and an intersected per-tool capability grant. Missing dependencies or grants fail closed. Control operations use stable idempotency keys, composite `(namespace_id, session_id, run_id)` targets, expected revisions, and fenced run ownership.

`delete_session` acquires a deletion fence and writes a tombstone. It does not purge retained evidence. The fence blocks new runs, async-subagent attempts, and result-triggered continuations before the host cancels owned work and completes the tombstone. Run interruption is cooperative; the bundle does not expose process termination or evidence purge.

## Product policy

| Product              | Query tools                          | Control tools                                                |
| -------------------- | ------------------------------------ | ------------------------------------------------------------ |
| CLI/TUI model        | Enabled for the selected local store | Not model-visible                                            |
| Standalone RPC agent | Enabled when granted by the profile  | Optional and profile-granted                                 |
| Generic SDK app      | Only with an injected query handle   | Only with an injected control handle and explicit operations |

Human CLI commands and external RPC methods remain separate product surfaces. For example, a user may delete a CLI session even though the model running inside CLI receives query-only authority.

RPC profile toolsets are explicit:

```toml
[profiles.operator]
model_id = "openai-responses:gpt-5"
toolsets = ["agent_session_query", "agent_session_control"]
```

Omit `agent_session_control` for read-only agents. A profile cannot gain control merely by guessing a tool name because execution also requires the host-bound scope and capability grant.

## Concurrency and durability

Only the owning product coordinator starts, steers, or interrupts runs. A durable `running` record alone does not imply that a run is locally controllable. Run admission enforces one active fenced lease per session, exact retries replay their receipt, and conflicting reuse of an idempotency key fails.

Late async-subagent results use the same admission boundary. RPC atomically links one delivered result to one new `async_subagent_result` continuation run instead of modifying the terminal parent run. See [Subagents](subagents.md) for the background execution lifecycle.
