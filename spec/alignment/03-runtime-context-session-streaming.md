# Runtime, Context, Session, and Streaming

## Scope

This document records only remaining runtime, context, session, and streaming gaps.

## Runtime Execution Decisions

- Public graph inspection is exposed through deterministic graph APIs, but mutable node-by-node execution control is not exposed.
- Node hook contexts are internal; richer pre/post node and event hooks remain a product decision.

Required direction:

- Decide whether live graph iteration, node override, and public node hook contexts are stable SDK surfaces.
- If yes, expose node contexts, retry policy, checkpoint state, cancellation semantics, and override behavior as typed contracts.

## Context State Gaps

- Blocking subagent child records are merged into parent stream collectors and live handles with source ids, but `agent_stream_queues` remains a placeholder for true real-time child queue ownership.

Required direction:

- Add stream queue ownership only if true real-time child interleaving becomes a product requirement.
- Add concurrent-run context tests before changing preparation semantics.

Current evidence:

- `HookedModel` consumes runtime request metadata through typed model execution hooks.
- `SubagentExecutionHook` consumes typed parent/child/task metadata around delegated child runs.

Rust-native decision:

- Context preparation remains mutable until Starweaver adopts a concurrent-run context contract.

## Live Streaming Gaps

| Reference behavior           | Remaining gap                                                                                                                                                                                                                                                                                                  | Required direction                                                                                                              |
| ---------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| Provider-native interruption | Model/provider stream cancellation is wired through shared `CancellationToken`, runtime request context, protocol clients, and shared HTTP/SSE transport. Tool cancellation uses the same token through `ToolContext`. Future provider adapters outside the shared HTTP/SSE path still need explicit adoption. | Require new provider adapters to consume the token before claiming stream-interrupt parity.                                     |
| Provider stream resume       | Retryable provider stream setup/body failures and clean closes without `FinalResult` reopen the incremental request and continue the run, with `model_stream_resume` sideband evidence. Future adapters that expose provider-private continuation cursors need adapter-specific fixtures.                      | Require future provider-private continuation adapters to preserve canonical history and stream evidence before claiming parity. |
| Subagent stream interleaving | Blocking delegated child records are merged into the parent stream with source ids after child completion; there is no async queue that forwards child records while the child run is still executing.                                                                                                         | Add queue ownership and live interleaving only if non-blocking subagent streaming becomes an SDK contract.                      |

## Durable Session And Storage Gaps

- Store-backed `AgentRuntimeBuilder` covers run persistence, completed live stream persistence through `finish_stream`, interrupted live stream cancelled-run persistence, stream archive projection, replay log events, approval/deferred decision persistence, and resume by session/run id.
- Runtime checkpoints record the latest emitted stream cursor when stream records are collected, so durable resume snapshots can replay from the checkpoint boundary instead of full run replay.
- Interrupted live streams append synthetic error tool returns for dangling response tool calls before exporting completion state.
- `AgentRuntime::restore_environment_from_state` restores exported provider state through `EnvironmentProviderFactoryRegistry`; portable defaults include virtual provider state and provider-scoped `ResourceRef` values, while local restore requires an explicit trusted-host policy factory. `AgentRuntime::restore_environment_from_state_with_resources` restores typed `ResourceRef` values through `ResourceRestoreFactoryRegistry` before provider restore.
- Durable live interruption recovery is connected to model/provider stream cancellation, running tool cancellation, retryable provider stream resume, and service-level replay evidence. `runtime_finish_stream_persists_interrupted_live_stream_recovery` and `runtime_durable_store_persists_provider_stream_resume_replay` cover interrupted terminal replay, provider-resume replay, and reconnecting-client replay-cursor transport.

Required direction:

- Add concrete browser, remote-storage, and media resource adapters once ownership, authentication, retention, and teardown rules are stable. Virtual process snapshots are exported and restored through `EnvironmentState`; live OS process reattachment remains a trusted host policy decision.
