# Provider SDK and HTTP Parameter Alignment Research

> Research date: 2026-06-16. Scope: evaluate whether Starweaver should adopt official OpenAI / Anthropic SDKs, or keep self-maintained HTTP mappers aligned to official docs and specs.

## Executive summary

OpenAI and Anthropic do not currently provide official Rust SDKs for their primary model APIs. OpenAI's official SDK page lists JavaScript/TypeScript, Python, .NET, Java, Go, Ruby, and CLI; Rust appears only under community libraries as `async-openai`, with OpenAI explicitly warning that community libraries are not verified for correctness or security. Anthropic's official SDK page lists Python, TypeScript, C#, Go, Java, PHP, and Ruby; Rust is not listed.

The recommended direction is therefore not to replace Starweaver's provider mappers with official Rust SDKs. There is no official Rust SDK to adopt. Instead, Starweaver should keep its provider-neutral AST, injectable HTTP transport, raw JSON request/response evidence, replay fixtures, and provider-specific escape hatches, while tightening mapper correctness against official contracts:

1. OpenAI: use the official OpenAI OpenAPI specification as a schema source for drift detection, DTO generation experiments, and fixture validation.
2. Anthropic: continue handwritten Messages API mapping, but align against official API reference pages and official non-Rust SDK behavior where docs are ambiguous.
3. Community Rust SDKs: use as references or optional feature-gated adapters only; do not make them the source of truth for Starweaver core behavior.

A focused local validation confirmed the current Starweaver replay and request-parameter test baseline passes: `cargo test -p starweaver-model --test replay --test request_parameters --locked` completed with 24 replay tests and 18 request parameter tests passing.

## Research questions

- Are there official OpenAI or Anthropic Rust SDKs that can replace our hand-written HTTP parameter mapping?
- If not, what official documentation/specification sources should constrain our implementation?
- What parts of Starweaver's current model layer make direct SDK adoption risky?
- What migration or validation path should we follow before implementing changes?

## Official SDK availability

| Provider  | Official Rust SDK | Official SDKs found                                      | Primary source                                                          | Practical conclusion                                                       |
| --------- | ----------------- | -------------------------------------------------------- | ----------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| OpenAI    | No                | JavaScript/TypeScript, Python, .NET, Java, Go, Ruby, CLI | [OpenAI SDKs and CLI](https://developers.openai.com/api/docs/libraries) | Use official OpenAPI spec and docs, not an official Rust SDK.              |
| Anthropic | No                | Python, TypeScript, C#, Go, Java, PHP, Ruby, CLI         | [Anthropic client SDKs](https://docs.anthropic.com/en/api/client-sdks)  | Use API reference and official SDK behavior as evidence; keep Rust mapper. |

OpenAI's SDK page explicitly lists Rust under "Community libraries" as `async-openai` and states that OpenAI does not verify the correctness or security of those projects. Anthropic's SDK page says its SDKs provide streaming, retries, and error handling, but Rust is not in the listed languages.

## Official contract sources

### OpenAI

OpenAI has an official OpenAPI repository and a raw `openapi.yaml` specification:

- [OpenAI OpenAPI repository](https://github.com/openai/openai-openapi)
- [Raw OpenAPI YAML](https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.yaml)
- [OpenAI SDKs and CLI](https://developers.openai.com/api/docs/libraries)

The SDK page also tells community library authors to watch the OpenAPI specification repository for API changes.

A local extraction from the official OpenAPI spec found these relevant request schemas:

```text
PATH /responses
operationId: createResponse
request schema: CreateResponse
property_count: 29
interesting present: model, input, instructions, previous_response_id, conversation, include, reasoning, text, tools, tool_choice, parallel_tool_calls, max_output_tokens, temperature, top_p, stream, stream_options, metadata, store, service_tier
interesting missing: max_completion_tokens, max_tokens, response_format, messages

PATH /chat/completions
operationId: createChatCompletion
request schema: CreateChatCompletionRequest
property_count: 35
interesting present: model, tools, tool_choice, parallel_tool_calls, max_completion_tokens, max_tokens, temperature, top_p, stream, stream_options, metadata, store, response_format, messages, service_tier
interesting missing: input, instructions, previous_response_id, conversation, include, reasoning, text, max_output_tokens
```

This confirms the important split Starweaver already models: Responses uses `input`, `instructions`, `text.format`, and `max_output_tokens`; Chat Completions uses `messages`, `response_format`, and `max_completion_tokens` / legacy `max_tokens`.

### Anthropic

Anthropic points users to its API reference for the full API specification, but no public official OpenAPI repository was found during this research. The authoritative sources for current Messages behavior are:

- [Anthropic client SDKs](https://docs.anthropic.com/en/api/client-sdks)
- [Create a Message API reference](https://platform.claude.com/docs/en/api/messages/create)
- [Messages streaming](https://docs.anthropic.com/en/api/messages-streaming)
- [Tool use overview](https://docs.claude.com/en/docs/build-with-claude/tool-use/overview)
- [Structured outputs](https://docs.claude.com/en/docs/build-with-claude/structured-outputs)

Important Anthropic API facts from those sources:

- `POST /v1/messages` uses top-level `system`; there is no `system` role in `messages`.
- Required core parameters include `model`, `messages`, and `max_tokens`.
- Common optional parameters include `temperature`, `top_p`, `top_k`, `stop_sequences`, `metadata`, `stream`, `tools`, `tool_choice`, and `thinking`.
- Tool calls are represented as `tool_use` blocks; tool results are sent back as `tool_result` blocks.
- Streaming uses SSE events: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, and `message_stop`.
- Tool-use streaming emits `input_json_delta.partial_json`.
- Extended thinking streaming emits `thinking_delta` and `signature_delta`.
- Structured outputs now use `output_config.format` with `type: "json_schema"`; strict tool use uses `strict: true` in tool definitions.

## Current Starweaver implementation inventory

Starweaver currently uses provider-neutral request/response types and handwritten provider wire mappers in `crates/starweaver-model`. This is consistent with the repository's model-layer spec in `spec/core/02-model-provider-replay.md`, which requires injectable HTTP clients, endpoint overrides, custom headers, extra body fields, replay fixtures, and raw request evidence.

### Main provider mapper files

| Provider protocol       | Request mapping                                                                          | Response parsing               | Streaming                                                                 |
| ----------------------- | ---------------------------------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------- |
| OpenAI Chat Completions | `crates/starweaver-model/src/providers/openai_chat.rs`                                   | `openai_chat.rs`               | No provider SSE parser; canonical stream fixtures only.                   |
| OpenAI Responses        | `crates/starweaver-model/src/providers/openai_responses/request.rs`                      | `openai_responses/response.rs` | `openai_responses/stream.rs` incremental parser.                          |
| Anthropic Messages      | `crates/starweaver-model/src/providers/anthropic/request.rs` and `anthropic/settings.rs` | `anthropic/response.rs`        | No provider SSE parser yet; protocol client falls back to final response. |
| Gemini generateContent  | `crates/starweaver-model/src/providers/gemini.rs`                                        | `gemini.rs`                    | No provider SSE parser yet.                                               |
| Bedrock Converse        | `crates/starweaver-model/src/providers/bedrock.rs`                                       | `bedrock.rs`                   | No provider SSE parser yet.                                               |

### Shared transport and request boundary

Relevant files:

- `crates/starweaver-model/src/transport/config.rs`
- `crates/starweaver-model/src/transport/client.rs`
- `crates/starweaver-model/src/providers/client/adapter_impl.rs`
- `crates/starweaver-model/src/providers/client/wire.rs`
- `crates/starweaver-model/src/providers/client/request_options.rs`

Important current capabilities:

- `HttpModelConfig` supports `base_url`, `endpoint_path`, `auth`, fixed headers, config-level `extra_body`, timeouts, retry policy, max-token parameter strategy, and metadata.
- `HttpRequestOptions` supports per-request headers, per-request `extra_body`, endpoint override, timeout override, and metadata.
- `build_http_request` merges request body in this order: provider mapper body, config `extra_body`, then request `extra_body`.
- `ModelHttpClient` is injectable and is used by tests, gateway routing, OAuth wrappers, and Bedrock/SigV4-style integration seams.
- `ProtocolModelClient` prepares canonical messages, builds provider wire JSON, applies output schema mapping, builds the HTTP request, finalizes provider-specific request details, checks the production-request guard, sends through retrying transport, and parses the provider response.

These boundaries are valuable and should remain even if generated DTOs or optional SDK adapters are introduced.

## Current parameter alignment assessment

### OpenAI Responses

Current implementation already aligns with major Responses concepts:

- `model` and `input` are generated in `openai_responses/request.rs`.
- System/instruction content is mapped to top-level `instructions`.
- Tool returns map to `type: "function_call_output"` with `call_id` and `output`.
- Server-side replay maps `previous_response_id`, `conversation`, provider item IDs, and encrypted reasoning inclusion.
- `thinking` maps to top-level `reasoning` with `effort` and optional `summary`.
- Function tools map to `type: "function"`, `name`, `description`, and `parameters`.
- Native tools are passed through via `NativeToolDefinition` into `tools`, which supports OpenAI web search, MCP, code interpreter, and other native tool shapes.
- Native JSON schema output maps to `text.format` in `providers/client/output_schema.rs`.
- Streaming supports raw Responses SSE event names and preserves canonical deltas and final result assembly.

Potential drift to audit:

- `apply_common_settings_inner` is shared and can write `presence_penalty`, `frequency_penalty`, `logit_bias`, and `stop`; each should be schema-checked for Responses before relying on it.
- `provider_options` is a useful escape hatch but can also hide schema drift; it should be covered by a known-field audit and fixture evidence.
- OpenAI Responses evolves quickly; relying only on handwritten tests without OpenAPI drift checks will miss new required fields or deprecations.

### OpenAI Chat Completions

Current implementation maps the classic Chat Completions shape:

- `messages` with `system`, `user`, `assistant`, and `tool` roles.
- `tools` as function tools under `type: "function"`.
- `tool_choice` variants for auto, none, required, and named functions.
- `response_format` for native JSON schema output.
- Chat response parsing handles text, refusal, tool calls, usage, provider id, and finish reason.

Potential drift to audit:

- Current common settings use `max_tokens`; the official OpenAPI schema includes both `max_completion_tokens` and legacy `max_tokens`. We should decide whether Starweaver should prefer `max_completion_tokens` for modern Chat models and retain `max_tokens` only as a compatibility override.
- Chat streaming is not implemented as a provider SSE parser. If Chat remains a supported first-class protocol, add a provider stream parser or explicitly document Responses as the only incremental OpenAI stream target.

### Anthropic Messages

Current implementation maps the basic Messages API well:

- Top-level `system` is generated from Starweaver system/instruction parts.
- `messages` only uses `user` and `assistant` roles.
- `max_tokens` defaults to `1024` when absent.
- `temperature`, `top_p`, `top_k`, `stop_sequences`, `thinking`, and `provider_options` are mapped.
- Text, image URL, document URL, base64 image, plain text document, `tool_use`, and `tool_result` blocks are represented.
- Anthropic thinking replay preserves `thinking` and `signature` when the provider is Anthropic.
- Prompt cache helper options map to `cache_control` on system/tool blocks.

Known gaps against current official docs:

- `output_config.format` structured output is supported by Anthropic docs, but `providers/client/output_schema.rs` currently does nothing for `ProtocolFamily::AnthropicMessages`.
- `tool_choice` is not natively mapped from Starweaver `ToolChoice` into Anthropic's `tool_choice`; it can only pass through today via `provider_options`.
- Strict tool use (`strict: true`) is not represented by current `ToolDefinition` mapping unless supplied through a schema/provider escape hatch.
- Anthropic streaming is documented in detail, but Starweaver currently has no Anthropic SSE parser and falls back to a final result for streaming calls.
- Server tools and MCP connector shapes should be modeled through `NativeToolDefinition` or provider-specific options, then covered by fixtures.

## Why direct SDK adoption is risky

Directly wrapping an SDK call around Starweaver's model layer would likely break important product requirements:

1. Exact raw JSON request evidence: replay tests compare expected provider JSON. SDKs often hide or normalize final request bodies.
2. Injectable transport: Starweaver uses `ModelHttpClient` for tests, gateways, OAuth, retry policy, Bedrock signing/gateway paths, and audit capture.
3. Endpoint and header overrides: Starweaver supports provider-compatible gateways and per-request routing.
4. Extra body and provider options: these are needed for prompt cache metadata, beta headers, gateway fields, and fast-moving provider options.
5. Provider-private metadata preservation: OpenAI Responses item IDs, encrypted reasoning, native tool opaque payloads, Anthropic thinking signatures, Bedrock additional response fields, and future unknown output items must survive into canonical response parts.
6. Codex/OAuth special handling: `OAuthBearerHttpClient` and Codex Responses body patching add non-standard headers, token refresh, stream-only behavior, and `store=false`/`instructions` patches. Official SDKs are unlikely to support this path directly.

SDKs can still be useful, but only behind a boundary that preserves Starweaver's final `HttpRequest` and `ModelResponse` contracts.

## Options considered

### Option A: Adopt official Rust SDKs

Not available. Neither OpenAI nor Anthropic currently publishes an official Rust SDK for these APIs.

Decision: reject.

### Option B: Adopt community Rust SDKs as the core provider layer

OpenAI's `async-openai` is a mature community library and may be generated from OpenAI's spec. Anthropic has several unofficial Rust SDKs, but they appear less mature and are not official.

Risks:

- Community SDKs are not verified by providers.
- They may lag behind fast-moving API fields.
- They can obscure raw request/response evidence.
- They can constrain Starweaver's provider-neutral AST and replay requirements.

Decision: do not use as core. Consider only as optional adapters or reference implementations.

### Option C: OpenAI spec-driven validation with existing mappers

Use the official OpenAI OpenAPI spec to constrain handwritten request/response mapping.

Recommended actions:

- Vendor or download a pinned OpenAI OpenAPI spec in a non-runtime validation path.
- Build a small schema extraction tool for `/responses` and `/chat/completions` request schemas.
- Add a test that compares Starweaver's known mapped fields against the official schema.
- Add schema-aware fixture validation for expected provider requests.
- Keep `extra_body` and `provider_options` as escape hatches, but annotate known provider-specific fields.

Decision: recommended.

### Option D: Anthropic docs-driven mapping with official SDK behavior samples

Because no official public OpenAPI spec was found for Anthropic, continue handwritten mapping but expand fixtures from official docs and official SDK examples.

Recommended actions:

- Add fixtures for `output_config.format`, strict tool use, native `tool_choice`, server tools, and Anthropic streaming event accumulation.
- Use official TypeScript/Go/Python SDK examples as behavior references where API docs are ambiguous.
- Implement Anthropic native structured output mapping and `tool_choice` mapping after fixture approval.

Decision: recommended.

## Recommended implementation plan after review

Phase 1: Contract audit only

- Add `memos/provider-sdk-alignment-research.md` as the research record.
- Add an OpenAI OpenAPI drift-check script or test under a non-runtime validation path.
- Produce a matrix of Starweaver fields vs official OpenAI/Anthropic fields.
- Do not change runtime behavior yet.

Phase 2: Low-risk mapper alignment

- OpenAI Chat: decide and fixture whether `max_completion_tokens` should become the default for Chat Completions, with `max_tokens` retained via `HttpModelConfig.max_tokens_parameter` or provider option.
- Anthropic: add native `output_config.format` mapping for `OutputMode::NativeJsonSchema`.
- Anthropic: add native `tool_choice` mapping from Starweaver `ToolChoice`.
- Add fixtures before changing each mapper.

Phase 3: Streaming and advanced provider capabilities

- Add Anthropic SSE parser fixtures for text deltas, `input_json_delta`, `thinking_delta`, `signature_delta`, cumulative usage, and error events.
- Decide whether OpenAI Chat streaming should remain canonical-only or get a provider parser.
- Extend native tool fixture coverage for OpenAI MCP and Anthropic server tools/MCP connector shapes.

Phase 4: Optional generated DTO or community SDK spike

- Experiment with generating OpenAI Rust DTOs from official OpenAPI, but use them only for validation or serialization comparison at first.
- If testing `async-openai`, isolate it behind a feature flag and require it to output or preserve final raw JSON, headers, endpoint override, stream events, and response metadata.
- Do not replace `ModelHttpClient` or replay evidence boundaries.

## Acceptance criteria for any future implementation

- Existing focused tests remain green:
  - `cargo test -p starweaver-model --test replay --test request_parameters --locked`
  - `cargo test -p starweaver-model --test stream_replay --locked`
  - `cargo test -p starweaver-model --test oauth_provider --locked`
- OpenAI provider request fields are checked against the official OpenAPI schema or an approved allowlist.
- Anthropic provider request fields are traceable to official API reference pages or official SDK examples.
- No change removes `extra_body`, `extra_headers`, endpoint override, request metadata, or injectable HTTP transport behavior.
- OpenAI Responses streaming still preserves raw item IDs, encrypted reasoning, function-call argument deltas, and final response fallback assembly.
- Codex/OAuth behavior is unchanged unless explicitly tested with `oauth_provider` fixtures.
- Provider-private unknown output items continue to be preserved as provider metadata or opaque parts rather than being dropped.

## Code validation performed

### OpenAI OpenAPI extraction

A local Python script downloaded the official OpenAI OpenAPI spec and extracted request-body fields for `/responses` and `/chat/completions`. The extraction confirmed the protocol split between Responses and Chat Completions and highlighted likely audit areas such as Chat `max_completion_tokens` vs legacy `max_tokens`.

### Starweaver replay/request baseline

Command:

```bash
cargo test -p starweaver-model --test replay --test request_parameters --locked
```

Result:

```text
running 24 tests
24 passed; 0 failed

running 18 tests
18 passed; 0 failed
```

This establishes the current behavior baseline before any mapper changes.

## Final recommendation

Do not wait for or depend on official Rust SDKs. They do not exist for OpenAI or Anthropic today. Starweaver should keep its self-maintained provider mappers, but make them more evidence-driven:

- OpenAI should become OpenAPI-spec-validated.
- Anthropic should become official-docs-and-fixtures-validated.
- Community SDKs should stay optional references, not core dependencies.
- The next implementation work should start with contract tests and fixture additions, then proceed to low-risk mapper fixes such as Anthropic `output_config.format`, Anthropic `tool_choice`, and OpenAI Chat token-parameter alignment.

## Sources

01. [OpenAI SDKs and CLI](https://developers.openai.com/api/docs/libraries) — official SDK language list, community Rust library listing, and OpenAPI repository reference.
02. [OpenAI OpenAPI repository](https://github.com/openai/openai-openapi) — official OpenAPI specification source.
03. [OpenAI raw OpenAPI YAML](https://raw.githubusercontent.com/openai/openai-openapi/master/openapi.yaml) — local schema extraction source.
04. [Anthropic client SDKs](https://docs.anthropic.com/en/api/client-sdks) — official SDK language list and API reference pointer.
05. [Anthropic Create a Message API reference](https://platform.claude.com/docs/en/api/messages/create) — Messages API request parameter and content block reference.
06. [Anthropic Messages streaming](https://docs.anthropic.com/en/api/messages-streaming) — SSE event flow, text deltas, tool input JSON deltas, and thinking/signature deltas.
07. [Anthropic tool use overview](https://docs.claude.com/en/docs/build-with-claude/tool-use/overview) — client tools, server tools, MCP connector pointer, tool choice, and strict tool use guidance.
08. [Anthropic structured outputs](https://docs.claude.com/en/docs/build-with-claude/structured-outputs) — `output_config.format`, strict tool use, SDK transformations, and schema limits.
09. `spec/core/02-model-provider-replay.md` — Starweaver model-layer responsibilities and replay contract.
10. `crates/starweaver-model/src/providers/*` — current provider mapper implementations.
11. `crates/starweaver-model/tests/replay.rs` and `crates/starweaver-model/tests/request_parameters.rs` — current replay and request-parameter test baseline.
