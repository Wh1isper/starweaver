# Trace Implementation Review

Date: 2026-06-11

## Scope

This memo reviews the current Starweaver trace implementation against:

- `spec/ops/05-observability.md` as the primary trace and observability contract.
- Related durability/session/stream expectations from `spec/ops/02-shared-execution-components.md`, `spec/ops/03-durable-service-runtime.md`, `spec/ops/04-cli-product.md`, and runtime/context seams described in `spec/core/01-agent-loop.md` and `spec/core/04-context-state-executor.md`.

The review focuses on runtime span semantics, context propagation, persistence correlation, exporter readiness, privacy policy, tests, and refactor needs.

## Executive Summary

The repository has a good trace foundation but it is not yet complete enough to be considered production observability.

Implemented foundations:

- `TraceContext` is shared in core and can be attached to SDK sessions, model requests, tool contexts, context snapshots, session records, and display records.
- Runtime has a small `TraceRecorder` abstraction, `SpanSpec`, `SpanEvent`, `SpanStatus`, `TraceLevel`, `InMemoryTraceRecorder`, `NoopTraceRecorder`, and an adapter seam.
- The main runtime loop creates agent, loop-step, model, tool, checkpoint, and history-compaction spans.
- Model and tool trace contexts are propagated to `ModelRequestContext` and `ToolContext`.
- Checkpoints carry resume trace context and trace metadata.
- There are tests for basic span nesting, model events, trace levels, and compaction spans.

Major gaps:

- Default info-level telemetry currently records full model requests/responses and tool arguments/results without a redaction or opt-in content policy.
- Span lifecycle is manually managed and not error-safe across many `?` paths; double-close and dangling spans are possible.
- Output validation spans and retry events are missing.
- OpenTelemetry GenAI and Starweaver correlation attributes are incomplete.
- Trace/span IDs are not OpenTelemetry/W3C-compatible by default.
- Session, stream, CLI, and storage surfaces have trace fields but are not consistently populated from active runtime trace context.
- `AdapterTraceRecorder` is currently only an in-memory wrapper, not an OpenTelemetry/OTLP/Langfuse adapter.
- Debug wire-level LLM tracing is only a metadata seam; raw provider request/response spans or events are not implemented.

Review opinion: keep the recorder abstraction and in-memory tests, but refactor the runtime tracing internals before adding real OTLP export. Exporting now would risk leaking content and producing incomplete or misleading traces.

## Current Implementation Map

### Core Trace Context

- `crates/starweaver-core/src/lib.rs` defines `TraceContext` with `trace_id`, `span_id`, `parent_span_id`, `trace_state`, and metadata.
- `TraceContext::from_trace_parent` accepts a W3C-like `traceparent` string but is permissive and does not validate fixed-width hex IDs.

Relevant files:

- `crates/starweaver-core/src/lib.rs:283`

### Runtime Recorder and Span Records

- `crates/starweaver-runtime/src/trace.rs` defines:
  - `TraceRecorder`
  - `SpanSpec`
  - `SpanEvent`
  - `SpanStatus`
  - `TraceLevel`
  - `SpanKind`
  - `RecordedSpan`
  - `SpanHandle`
  - `InMemoryTraceRecorder`
  - `NoopTraceRecorder`
  - `AdapterTraceRecorder`
- `InMemoryTraceRecorder` creates child contexts, preserves trace ID, creates fresh span IDs, stores parent span IDs, records events, and closes spans.
- `NoopTraceRecorder` returns the parent context unchanged.
- `AdapterTraceRecorder` is a placeholder seam backed by `InMemoryTraceRecorder`.

Relevant files:

- `crates/starweaver-runtime/src/trace.rs`

### Runtime Span Shape

The main run loop creates:

- Agent span: `gen_ai.invoke_agent`
- Loop step spans: `starweaver.loop.step`
- Model spans: `gen_ai.inference` with client span kind
- Tool spans: `gen_ai.execute_tool`
- Checkpoint spans: `starweaver.checkpoint`
- History compaction spans: `starweaver.history.compaction`

Relevant files:

- `crates/starweaver-runtime/src/agent/run_loop.rs:311`
- `crates/starweaver-runtime/src/agent/run_loop.rs:353`
- `crates/starweaver-runtime/src/agent/run_loop.rs:423`
- `crates/starweaver-runtime/src/agent/run_loop.rs:776`
- `crates/starweaver-runtime/src/agent/runtime_helpers.rs:258`
- `crates/starweaver-runtime/src/agent/runtime_helpers.rs:374`

### Model Trace Events

Model spans record:

- `starweaver.model.request`
- `starweaver.model.stream_event`
- `starweaver.model.response`

`ModelRequestContext` receives the model span trace context.

Relevant files:

- `crates/starweaver-runtime/src/agent/run_loop.rs:423`
- `crates/starweaver-runtime/src/agent/runtime_helpers.rs:398`
- `crates/starweaver-model/src/adapter.rs`

### Tool Trace Events

Tool spans record:

- `starweaver.tool.call`
- `starweaver.tool.return`

`ToolContext` receives the tool span trace context and retry budget.

Relevant files:

- `crates/starweaver-runtime/src/agent/run_loop.rs:776`
- `crates/starweaver-tools/src/context.rs`

### SDK and Subagent Propagation

- `AgentSession::with_trace_context` and `AgentSession::with_trace_parent` can seed the runtime context.
- Subagent contexts inherit the parent trace context via `parent_context.subagent_context(name)`.
- Child agents can therefore create nested `gen_ai.invoke_agent` spans if they use the same recorder.

Relevant files:

- `crates/starweaver-agent/src/session.rs:174`
- `crates/starweaver-agent/src/subagent.rs:621`
- `crates/starweaver-context/src/lib.rs:1117`

### Durable and Stream Structures

Trace fields exist in session and stream contracts:

- `SessionRecord.trace_context`
- `RunRecord.trace_context`
- display message/projection trace contexts
- context snapshots and resumable state trace snapshots

However, CLI/local store paths do not consistently populate these fields from runtime execution.

Relevant files:

- `crates/starweaver-session/src/records.rs:154`
- `crates/starweaver-session/src/records.rs:212`
- `crates/starweaver-stream/src/display.rs:145`
- `crates/starweaver-cli/src/local_store.rs:398`
- `crates/starweaver-cli/src/local_store.rs:421`

### Current Tests

Covered:

- Basic nested runtime span tree.
- Model request/stream/response events.
- History compaction span.
- Debug trace levels.
- Adapter seam smoke test.

Relevant files:

- `crates/starweaver-runtime/tests/trace.rs`
- `crates/starweaver-runtime/tests/trace_model.rs`

## Spec Coverage Matrix

| Spec expectation                    | Current status | Notes                                                                                                    |
| ----------------------------------- | -------------- | -------------------------------------------------------------------------------------------------------- |
| External parent trace accepted      | Partial        | SDK session supports `with_trace_parent`; CLI/service path is not consistently wired.                    |
| Agent span                          | Partial        | Exists, but attributes and lifecycle error safety are incomplete.                                        |
| Loop-step span                      | Partial        | Exists; active span context is not explicit across all helpers.                                          |
| Model request span                  | Partial        | Exists with canonical events; content policy and response attributes are incomplete.                     |
| Tool execution span                 | Partial        | Exists; content policy, lifecycle, retry events, and some attributes are incomplete.                     |
| Output validation span              | Missing        | `starweaver.output_validation` is not created.                                                           |
| Checkpoint span and resume evidence | Partial        | Span and metadata exist; persistence correlation needs end-to-end tests.                                 |
| Subagent nested span tree           | Partial        | Context inheritance exists; explicit delegation span/attributes and recorder inheritance need hardening. |
| Retry span events                   | Missing        | Runtime emits stream/context retry events, not trace retry events.                                       |
| Info/debug trace levels             | Partial        | Types exist; default info currently includes full content.                                               |
| Content redaction and opt-in export | Missing        | Highest privacy risk.                                                                                    |
| OTel GenAI attributes               | Partial        | A few attributes exist; many required fields are missing.                                                |
| Starweaver correlation attributes   | Partial        | Run/session/conversation/agent/checkpoint/stream correlation is sparse.                                  |
| SessionStore trace persistence      | Partial        | Fields exist; active trace context is not consistently persisted.                                        |
| In-memory recorder tests            | Partial        | Basic coverage exists; more acceptance-gate tests are needed.                                            |
| OpenTelemetry/OTLP exporter         | Missing        | Adapter seam is placeholder only.                                                                        |
| Langfuse adapter                    | Missing        | No mapping or snapshot tests yet.                                                                        |
| Debug provider wire tracing         | Missing        | Metadata seam exists, but no raw HTTP request/response events.                                           |

## Findings

### P0. Default trace content export violates privacy policy

Current info-level trace events include full canonical model messages, model responses, tool call arguments, and tool results:

- `record_model_request_event` stores `gen_ai.request` with `messages`, `settings`, and `params`.
- `record_model_response_event` stores full `gen_ai.response`.
- Tool spans store `gen_ai.tool.call.arguments` and `gen_ai.tool.call.result`.

This conflicts with `spec/ops/05-observability.md`, which says content-bearing fields are opt-in and must pass through redaction/truncation policy.

Risk:

- Prompts, secrets, filesystem content, tool outputs, model outputs, and provider data can leak into default traces.
- Once OTLP export is added, this becomes a production data-handling issue.

Recommended fix:

- Introduce `TraceContentPolicy` and a redaction layer before recording any content-bearing attributes.
- Default info-level events should record structural metadata only:
  - message counts
  - tool counts
  - native tool counts
  - output-schema presence
  - usage
  - finish reasons
  - hashes or previews only if policy allows
- Require explicit opt-in for:
  - `gen_ai.system_instructions`
  - `gen_ai.input.messages`
  - `gen_ai.output.messages`
  - `gen_ai.tool.definitions`
  - `gen_ai.tool.call.arguments`
  - `gen_ai.tool.call.result`
- Add JSON-path redaction, truncation limits, media reference substitution, and per-tool rules.

### P0. Span lifecycle is not error-safe

Spans are currently opened and closed manually in the run loop. Many capability hooks and helper calls use `?` after spans are open. Some paths can return without closing the active run, loop-step, model, tool, or checkpoint span. Some tool paths can close the same tool span once before `call_after_tool_result` and again on retry-limit logic.

Risk:

- Dangling open spans.
- Missing error status on failed paths.
- Double-close ambiguity in in-memory and future exporter implementations.
- Trace viewers may show misleading successful or unfinished runs.

Recommended fix:

- Add span lifecycle helpers instead of ad-hoc close calls.
- Options:
  - `TraceSpanGuard` with explicit `finish_ok` / `finish_error` / `finish_status` methods.
  - `with_span` / `try_with_span` async helpers that map returned errors to span status.
  - An idempotent close implementation in recorders to prevent double-close from corrupting state.
- Make every runtime boundary close exactly once.
- Ensure span status includes `error.type` semantics.
- Add failure-injection tests for model errors, tool errors, capability hook failures, output validation failure, checkpoint failure, and suspension.

### P0. Output validation spans are missing

`validate_final_output` performs parsing, output validators, capability validation hooks, and after-validation hooks, but it does not create `starweaver.output_validation`.

Risk:

- Trace shape does not match the spec.
- Output retries and validation failures are hard to inspect.

Recommended fix:

- Create `starweaver.output_validation` under the active loop-step span.
- Add attributes:
  - `starweaver.output.has_schema`
  - `starweaver.output.validator.count`
  - `starweaver.output.length` or structural summary
  - `starweaver.run.step`
- Record success/error status and validation error type.
- Do not record full output unless content policy allows it.

### P0. Retry trace events are missing

The runtime publishes retry stream/context events but does not record retry span events.

Missing trace events include:

- Model error retry.
- Output retry.
- Tool retry.
- Retry limit exceeded events.

Recommended fix:

- Add `starweaver.retry` span events on the active loop-step span and, where applicable, child model/tool/output-validation spans.
- Attributes should include:
  - retry kind
  - retry count
  - max retries
  - error type
  - recovery changed flag for model history recovery
  - tool name/call ID for tool retries
- Add tests for output retry and tool retry span events.

### P1. Standard attributes are incomplete

The spec requires OpenTelemetry GenAI attributes and Starweaver correlation attributes. Current spans include only a subset.

Common missing attributes:

- `gen_ai.conversation.id`
- `gen_ai.agent.id`
- `gen_ai.agent.name`
- `gen_ai.agent.description`
- `gen_ai.agent.version`
- `gen_ai.response.model`
- `gen_ai.response.finish_reasons`
- `gen_ai.usage.cache_read.input_tokens`
- `gen_ai.usage.cache_creation.input_tokens`
- `error.type`
- `starweaver.session.id`
- `starweaver.run.id`
- `starweaver.parent_run.id`
- `starweaver.conversation.id`
- `starweaver.agent.id`
- `starweaver.agent.name`
- `starweaver.subagent.name`
- `starweaver.checkpoint.id`
- `starweaver.environment.provider.id`
- `starweaver.stream.cursor`

Recommended fix:

- Centralize span construction in typed builders:
  - `AgentSpanBuilder`
  - `LoopStepSpanBuilder`
  - `ModelSpanBuilder`
  - `ToolSpanBuilder`
  - `OutputValidationSpanBuilder`
  - `CheckpointSpanBuilder`
  - `SubagentSpanBuilder`
- Builders should enforce low-cardinality OTel and Starweaver attributes at span creation time.
- Runtime should provide a `TraceCorrelation` object derived from context/state/session metadata.

### P1. Trace/span ID generation is not OpenTelemetry-compatible

`InMemoryTraceRecorder` currently generates IDs like `trace_<uuid>` and `span_<uuid>`. OTel requires fixed-width lowercase hex IDs:

- Trace ID: 32 hex characters.
- Span ID: 16 hex characters.

`TraceContext::from_trace_parent` is permissive and does not reject malformed W3C traceparent values.

Risk:

- Real OTLP adapters will need translation or may reject IDs.
- Parent propagation tests can pass while production propagation fails.

Recommended fix:

- Generate OTel-compatible IDs in recorders.
- Strictly parse W3C `traceparent`:
  - version is two hex chars
  - trace ID is 32 lowercase hex chars and not all zero
  - parent span ID is 16 lowercase hex chars and not all zero
  - flags are two hex chars
- Preserve backward compatibility by offering an explicit fallback for non-W3C trace IDs if needed, but do not treat arbitrary strings as valid traceparent.

### P1. Session, stream, CLI, and storage trace persistence is incomplete

Trace fields exist in session and stream structures, but active runtime trace context is not consistently copied into durable records or display projections.

Observed issues:

- `RunRecord::new` initializes `trace_context` to default.
- CLI local store `append_run` and `complete_run` update many fields but do not clearly set `run.trace_context` or `session.trace_context` from the active runtime state.
- Display projection context has a trace context field, but the CLI projection path appears to use a default trace context.
- `AgentStreamRecord` itself does not carry trace context, which makes stream-to-span correlation weaker.

Recommended fix:

- Persist the active agent span context at run start.
- Persist final session trace context at run completion or suspension.
- Attach active trace context and stream cursor to display messages and raw stream records.
- Add compact trace projection records that can join session/run/checkpoint/stream data with external trace IDs.
- Add end-to-end tests through the session/local store path.

### P1. Subagent trace semantics need hardening

Current subagent context inheritance is a useful foundation, but it is not enough for the full spec.

Gaps:

- No explicit parent-side subagent delegation span/event with `starweaver.subagent.name` and task ID.
- Recorder inheritance is implicit through cloned agents; it should be explicit or enforced.
- Parent run ID and child run ID correlation are event-level but not span-level.
- Nested subagent span-tree snapshot tests are missing.

Recommended fix:

- Add a subagent delegation span around the parent-side delegation call.
- Ensure the child agent receives the same `DynTraceRecorder` unless explicitly overridden.
- Add attributes:
  - `starweaver.subagent.name`
  - `starweaver.subagent.task.id`
  - `starweaver.parent_run.id`
  - child `starweaver.run.id` when known
- Add nested subagent tests using `InMemoryTraceRecorder`.

### P1. Active span context is not explicit enough

Helpers such as history compaction start spans from `context.trace_context`, which generally points to the agent span, not necessarily the currently active loop-step span.

Risk:

- Some child spans may attach to the agent span rather than the intended active loop step.
- Future capability/filter spans may be inconsistently nested.

Recommended fix:

- Introduce an explicit active span context or span stack for runtime internals.
- Pass active parent span context to helpers that create child spans.
- Keep `AgentContext.trace_context` for propagation/correlation, but avoid using it as an implicit active span stack.

### P2. Exporter adapters are placeholders

`AdapterTraceRecorder` is currently backed by in-memory storage and does not export to `tracing`, OpenTelemetry, OTLP, or Langfuse-compatible metadata.

Recommended fix:

- Keep `TraceRecorder` in runtime as the core abstraction.
- Add feature-gated adapters after P0 privacy/lifecycle fixes:
  - `tracing` adapter
  - OpenTelemetry SDK adapter
  - OTLP exporter adapter
  - Langfuse metadata mapper
- Add snapshot tests for Langfuse-friendly metadata and an integration test behind a feature flag for OTLP export.

### P2. Debug wire-level LLM tracing is not implemented

The model layer supports `llm_trace_metadata`, and provider clients can merge metadata into HTTP options, but there is no debug recorder for exact provider request/response bodies or raw stream chunks.

Recommended fix:

- Extend `ModelRequestContext` with optional debug trace recorder or debug event sink.
- Record debug-level provider events only when enabled by policy:
  - converted HTTP request body after provider adapter conversion
  - headers/options after safe redaction
  - raw HTTP response before canonical parsing
  - raw stream chunks before canonical normalization
- Ensure debug events use stricter redaction and sampling than info-level telemetry.

## Recommended Refactor Plan

### Phase 1: Safety and Spec Semantics

Goal: make in-memory traces correct and safe before external export.

Action items:

1. Add `TraceContentPolicy` and default structural-only event recording.
2. Refactor span lifecycle with guards/helpers and idempotent close semantics.
3. Add `starweaver.output_validation` span.
4. Add retry span events for model, output, and tool retry paths.
5. Add strict OTel-compatible trace/span ID generation and traceparent parsing.
6. Add typed span attribute builders for required OTel GenAI and Starweaver correlation attributes.
7. Add tests for privacy defaults, span closure on errors, output validation, retries, and ID format.

### Phase 2: Correlation and Persistence

Goal: make traces joinable with sessions, runs, streams, and checkpoints.

Action items:

1. Persist active agent trace context into `RunRecord.trace_context` at run start.
2. Persist final session trace context into `SessionRecord.trace_context`.
3. Add trace context to raw runtime stream records or a parallel trace projection.
4. Populate display projection trace context and stream cursor attributes.
5. Add checkpoint trace persistence tests.
6. Add CLI/local store integration tests for trace context persistence.
7. Add subagent delegation spans and nested subagent snapshot tests.

### Phase 3: Export and Backend Integration

Goal: support production observability backends.

Action items:

1. Implement feature-gated OpenTelemetry/OTLP recorder adapter.
2. Implement Langfuse-friendly metadata mapping.
3. Add sampling hooks at span creation.
4. Add debug LLM wire-level tracing behind explicit opt-in.
5. Add exporter integration tests behind feature flags.
6. Document setup and privacy defaults in user-facing docs after behavior is stable.

## Proposed Acceptance Tests to Add

P0 tests:

- `trace_parent_propagates_to_agent_span`
- `trace_ids_are_otel_compatible`
- `malformed_traceparent_is_rejected_or_falls_back_explicitly`
- `info_trace_omits_content_by_default`
- `content_trace_requires_opt_in_and_redacts_paths`
- `spans_close_with_error_when_model_fails`
- `spans_close_with_error_when_tool_fails`
- `spans_close_with_error_when_capability_hook_fails`
- `output_validation_span_records_success`
- `output_validation_span_records_error`
- `retry_events_are_recorded_for_output_retry`
- `retry_events_are_recorded_for_tool_retry`

P1 tests:

- `checkpoint_resume_evidence_contains_trace_context`
- `session_store_persists_run_trace_context`
- `session_store_persists_session_trace_context`
- `display_messages_include_trace_context`
- `stream_records_correlate_to_trace_context`
- `nested_subagent_spans_share_trace_id_and_parent_correctly`
- `subagent_delegation_span_records_task_and_child_run`

P2 tests:

- `langfuse_adapter_maps_span_roles_snapshot`
- `otlp_exporter_emits_valid_trace_under_feature_flag`
- `debug_llm_wire_trace_records_redacted_request_response_when_enabled`

## Suggested Action Item Backlog

### P0: Must complete before real export

- [ ] Implement trace content policy with safe defaults.
- [ ] Remove full request/response/tool content from default info events.
- [ ] Add span lifecycle guard/helper and close spans safely on all paths.
- [ ] Add output validation spans.
- [ ] Add retry span events.
- [ ] Generate OTel-compatible trace/span IDs.
- [ ] Strictly parse W3C traceparent.
- [ ] Centralize standard trace attributes.
- [ ] Add P0 acceptance tests.

### P1: Required for durable runtime quality

- [ ] Persist run/session trace context through session store and CLI local store.
- [ ] Correlate stream/display messages with trace context and stream cursors.
- [ ] Add explicit subagent delegation spans and recorder inheritance tests.
- [ ] Make active span parent explicit in runtime helpers.
- [ ] Add checkpoint resume evidence tests.
- [ ] Add nested subagent span-tree tests.

### P2: Backend integration and advanced diagnostics

- [ ] Implement OpenTelemetry/OTLP recorder adapter behind feature flag.
- [ ] Implement Langfuse metadata adapter and snapshots.
- [ ] Add sampling hooks.
- [ ] Add debug provider wire-level trace events behind opt-in policy.
- [ ] Add docs for observability setup, privacy, and trace correlation.

## Bottom Line

The current trace implementation is a solid in-memory and SDK-level foundation. It already proves the desired runtime span skeleton and propagation seams. The next work should not start with exporters. It should first close the privacy, lifecycle, output-validation, retry, ID-format, and persistence gaps. After those are fixed and covered by tests, adding OTLP and Langfuse adapters will be much lower risk.
