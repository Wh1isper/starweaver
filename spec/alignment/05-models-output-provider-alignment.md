# Models, Output, and Provider Alignment

## Scope

This document tracks only remaining provider, model, output, usage, and media gaps.

## Provider Replay Evidence

| Provider area                     | Evidence                                                                                                                                                      |
| --------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| OpenAI Responses continuation     | Previous-response, conversation, and compaction-boundary fixtures prove server-side continuation trimming and compaction-boundary behavior.                   |
| Anthropic private thinking replay | `anthropic/provider_thinking_replay.json` proves provider-owned thinking signatures replay as native Anthropic `thinking` blocks after fixture-state restore. |

Required direction:

- For future provider adapters, add private continuation fixtures only where the provider exposes durable replay identifiers or payloads.
- Keep provider fixture failures actionable with normalized diffs.

## Structured Output Evidence

- `provider_requests_include_prompted_output_retry_diagnostics` proves prompted-output schema instructions and retry diagnostics survive OpenAI Chat, OpenAI Responses, Anthropic, Gemini, and Bedrock provider request mapping.

Rust-native decision:

- Multi-output selector semantics remain a product choice; current typed output, output functions, and `AgentEndStrategy` cover adopted output behavior.

## Usage And Pricing Evidence

- Built-in pricing catalog ownership is release-bound, with source URLs and last-checked dates next to catalog entries.
- Contract billing overrides use caller-provided `PricingEstimate` values or caller-owned `CostBudget`.
- Usage snapshot stream events carry typed stable fields for UI, replay, and transport clients.
- External non-model usage can be included through `AgentContext::update_external_usage_snapshot_entry`.

## Media And Output Gaps

- External media/resource store adapters need durable resource records beyond the current provider-scoped or host-restored `ResourceRef` and output URL wrappers.
- Provider input fixtures now cover OpenAI file inputs, Anthropic document inputs, Gemini audio/video/document inputs, Bedrock resource-backed document inputs, and OpenAI Responses generated image/file output.
- Durable generated-file outputs beyond OpenAI Responses image/file items are not proven across adapters.

Required direction:

- Add external durable media resource records once the resource-store ownership contract is stable.
- Add provider fixtures for generated audio, video, document, and non-image file outputs only where adapters support those output forms.

Current evidence:

- Runtime trace records can be projected through `export_otel_gen_ai_spans` / `AdapterTraceRecorder::otel_gen_ai_spans`, with tests covering agent, model, tool, response, finish reason, usage, and cache-usage GenAI fields.
- Provider request audit snapshots are captured through the explicit `ProtocolModelClient::with_provider_request_audit` path, separate from redacted trace spans. Audit policy can capture metadata only, redacted payloads, or full payloads for local fixture/debug work.
- OpenAI Responses replay options are typed through `ProviderReplaySettings`; raw `openai_*` replay alias inputs are not accepted as request settings.
- `provider_requests_are_stable_after_restored_fixture_state` rebuilds every provider fixture after JSON restore of history, settings, tools, and native tools; normalized request mismatches print pretty expected and actual JSON.
- `provider_requests_preserve_representative_compacted_history_shape` proves representative compacted history preserves assistant summary and restored request order across OpenAI Chat, OpenAI Responses, Anthropic, Gemini, and Bedrock provider mappers.
- `openai_responses/previous_response_auto_trim.json`, `openai_responses/conversation_auto_trim.json`, and `openai_responses/previous_response_compaction_boundary.json` prove OpenAI Responses server-side continuation trimming and compaction-boundary behavior remain stable after fixture-state restore.
- `anthropic/provider_thinking_replay.json` and `anthropic_private_thinking_replay_fixture_maps_signature_natively` prove same-provider Anthropic private thinking signatures replay as native provider thinking blocks and survive fixture-state restore.
- `openai_chat/prompt_cache_settings.json` and `openai_responses/prompt_cache_settings.json` prove typed OpenAI prompt-cache settings map to provider request bodies.
- `gemini/generation_config_seed_topk.json` proves Gemini `topK`, `seed`, cached content, and logprob settings map to provider request bodies; request-parameter tests cover Google Cloud service-tier routing headers.
- `bedrock/typed_request_fields.json` proves Bedrock guardrail, performance, request metadata, response field paths, prompt variables, and inference profile settings map to provider request bodies.
- `anthropic/tool_choice_parallel_policy.json` proves Anthropic named tool choice, disabled parallel tool use, context management, container, service tier, and tool schema mapping.
- Media request fixtures cover OpenAI Chat/Responses file inputs, Anthropic document inputs, Gemini audio/video/document inputs, and Bedrock resource-backed document inputs.
- `builder_default_media_upload_filter_uses_configured_uploader_without_duplicate_id` proves the SDK media upload filter can replace inline media with a provider-scoped `ResourceRef`; `resource_restore_registry_restores_typed_resources_and_preserves_provider_refs` and `runtime_builder_owns_session_state_environment_and_streaming` prove typed external `ResourceRef` values can be host-restored before provider restore; `builder_default_media_upload_filter_keeps_original_media_on_upload_failure` proves upload failures keep original inline media and publish request diagnostics.
- `agent_result_exposes_image_output_wrappers` and the output docs prove generated file response parts are exposed through `OutputMedia` wrappers for image/media output policies.
- `codex_oauth_streaming_model_builds_subscription_request_shape` proves the real Codex OAuth streaming model path applies subscription request body requirements, OAuth/account headers, User-Agent, typed session/thread routing headers, `x-client-request-id`, and metadata propagation.
- Existing Gateway/Codex request-parameter tests prove typed gateway sticky routing, case-insensitive header override, typed Codex metadata, explicit Codex routing header preservation, and generated routing-header exclusion when a non-canonical alias is supplied.
