# Agent SDK Surface Parity

## Scope

This document tracks only remaining differences in Starweaver's agent construction, streaming, runtime, and related SDK surfaces.

## `create_agent(...)` Status

- No remaining `create_agent(...)` gap is tracked for current SDK scope.
- MCP approval behavior maps to Starweaver's Rust-native `approval_required_tools(...)` / `ApprovalRequiredToolset` policy over MCP-discovered tool names. `runtime_durable_store_resumes_live_mcp_approval_and_deferred_records` proves live MCP approval records, deferred records, resume, and resumed model history at the host-backed live MCP seam; `runtime_durable_store_resumes_rmcp_stdio_approval_and_deferred_records` proves the same flow over the concrete `RmcpLiveMcpClient` stdio protocol path.

## `AgentRuntime` Gaps

- Durable builder bindings exist for `SessionStore`, `StreamArchive`, and replay logs. Live interruption, provider stream resume, HITL resume, replay-cursor transport, and typed resource restore seams now have service-level evidence; the remaining runtime gap is concrete external resource adapter policy.

## `stream_agent(...)` Gaps

- `AgentStreamHandle::interrupt()` now propagates a shared `CancellationToken` through runtime request context, model event streams, protocol clients, the shared HTTP/SSE reqwest transport, and `ToolContext`. Remaining cancellation work is limited to future provider adapters that bypass the shared HTTP/SSE path.
- Retryable provider stream setup/body failures and clean stream closes without a final result now reopen the incremental request and continue the run, with `model_stream_resume` sideband evidence.
- Parent live streams merge blocking subagent child records with source ids after the child run completes, but there is no true real-time child queue interleaving while the child run is still executing.

Current evidence:

- `HookedModel` and `ModelExecutionHook` wrap any model adapter with typed metadata containing model name, provider name, run id, conversation id, streaming flag, agent metadata, request parameters, and final response.
- `SubagentExecutionHook` wraps delegated child runs with typed metadata, mutable child context access before execution, and completed/failed outcomes carrying output, run id, and usage.
- `session_live_stream_interrupt_cancels_model_stream_token` proves SDK stream interruption cancels the model request token before the timeout-backed abort path.
- `session_live_stream_interrupt_cancels_running_tool_token` proves SDK stream interruption cancels a running tool through `ToolContext`.
- `provider_stream_transport_error_resumes_incremental_request` proves retryable provider stream failures resume the incremental request and emit `model_stream_resume`.
- `provider_stream_clean_close_without_final_resumes_incremental_request` proves provider streams that close without `FinalResult` resume through the same event path.
- `runtime_finish_stream_persists_interrupted_live_stream_recovery` proves store-backed `finish_stream` persists interrupted live stream context, cancelled run status, raw records, replay terminal markers, and resume-snapshot evidence before returning `AgentStreamError::Interrupted`.
- `runtime_durable_store_persists_provider_stream_resume_replay` proves store-backed provider stream resume persists `model_stream_resume` sideband evidence, raw stream records, archive records, replay terminal markers, reconnecting-client replay-cursor transport, and checkpoint-boundary resume snapshots.
- `sdk_subagent_registry_supports_multi_level_nested_delegation` proves nested child-to-grandchild delegation preserves lifecycle events and nested stream source attribution.

## Rust-Native Decisions

- Model string resolution, generic `extra_context_kwargs`, direct `runtime.ctx`, and one-call async constructor semantics are Python SDK conveniences. Starweaver keeps explicit model adapters, typed context setters, `AgentSession`, and `AgentRuntimeBuilder` unless a host product chooses a convenience facade.
- Exact Python `output_type` constructor symmetry is not a target; Rust-native typed output, output functions, and `AgentEndStrategy` are the adopted contracts.
- Capability hooks are the Starweaver-native lifecycle extension model; mirroring the reference extension object model requires a separate product decision.
- `prepare_new_run()` mutates `AgentContext` by design. A fresh per-run context copy would require a concurrent-run context contract first.

## Acceptance

- Any added SDK surface remains Rust-native and uses Starweaver-native names.
- Durable runtime, stream, and wrapper APIs have integration tests covering run, interrupt, resume, and replay when introduced.
