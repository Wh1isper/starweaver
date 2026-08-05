# Models

`starweaver-model` defines provider-neutral model primitives and provider protocol clients.

## Deterministic test model

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, TestModel};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = Arc::new(TestModel::with_text("deterministic"));
let agent = AgentBuilder::new(model).build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "deterministic");
# Ok(())
# }
```

## Function model

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, FunctionModel};
use starweaver_model::{latest_user_text, ModelResponse};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = FunctionModel::new(|messages, _settings, _info| {
    let prompt = latest_user_text(&messages).unwrap_or_default();
    Ok(ModelResponse::text(format!("echo: {prompt}")))
});
let agent = AgentBuilder::new(Arc::new(model)).build();

let result = agent.run("hello").await?;
assert_eq!(result.output, "echo: hello");
# Ok(())
# }
```

## Streaming test model

`TestModel` and `FunctionModel` support both `request` and `request_stream`, so runtime streaming tests can exercise provider delta events without a production model.

```rust
use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentStreamEvent, TestModel};
use starweaver_model::{
    ModelResponse, ModelResponseStreamEvent, PartDelta, PartEnd, PartStart, StreamDelta,
};

# async fn example() -> Result<(), starweaver_agent::AgentError> {
let model = TestModel::with_stream_events(vec![vec![
    ModelResponseStreamEvent::PartStart(PartStart {
        index: 0,
        part_kind: "text".to_string(),
    }),
    ModelResponseStreamEvent::PartDelta(PartDelta {
        index: 0,
        delta: StreamDelta::Text {
            text: "hel".to_string(),
        },
    }),
    ModelResponseStreamEvent::PartEnd(PartEnd {
        index: 0,
        part_kind: Some("text".to_string()),
    }),
    ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("hello"))),
]]);
let agent = AgentBuilder::new(Arc::new(model)).build();

let stream = agent.run_stream("hello").await?;
assert!(stream.events().iter().any(|record| matches!(
    record.event,
    AgentStreamEvent::ModelStream { .. }
)));
# Ok(())
# }
```

## Built-in model presets

`starweaver-model` includes built-in presets for common provider settings and model capability profiles. The preset names mirror the SDK-facing Starweaver style: provider defaults such as `anthropic`, effort presets such as `openai_responses_high` or `grok_4_5_high`, the GPT-5.6 Pro reasoning-mode preset `openai_responses_pro`, and capability presets such as `claude_1m`, `gpt5_270k`, `gpt5_350k`, or `grok_4_5_500k`.

```rust
use starweaver_agent::{get_model_config, get_model_settings};

# fn example() -> Result<(), starweaver_agent::ModelPresetError> {
let settings = get_model_settings("openai_responses_high")?;
assert_eq!(settings.max_tokens, Some(32 * 1024));

let pro = get_model_settings("openai_responses_pro")?;
assert_eq!(pro.thinking.unwrap().mode.as_deref(), Some("pro"));

let config = get_model_config("claude")?;
assert_eq!(config.context_window, 1_000_000);

let subscription = get_model_config("gpt5_350k")?;
assert_eq!(subscription.context_window, 350_000);

let grok = get_model_config("grok-4.5")?;
assert_eq!(grok.context_window, 500_000);
# Ok(())
# }
```

`openai_responses_pro` targets GPT-5.6 on the Responses API. It sends `reasoning.mode = "pro"` with medium reasoning effort. Existing `openai_responses_*` effort presets omit `reasoning.mode`, preserving the API's default `standard` mode and compatibility with models that do not support reasoning mode.

`ModelSettings::merge` replaces `thinking` as one nested value. When overlaying a Pro preset with a different effort, repeat `mode: pro` in the overlay so the Pro mode is preserved.

Agent specs can reference a settings preset and optionally overlay request settings:

```yaml
name: coding-agent
model:
  model_id: claude-sonnet
  settings_preset: anthropic_high
  settings:
    max_tokens: 4096
```

For production aliases, combine a runtime preset with a provider HTTP config:

```rust
use starweaver_agent::{
    anthropic_http_config, model_runtime_preset, xai_responses_http_config,
};

# fn example() -> Result<(), starweaver_agent::ModelPresetError> {
let preset = model_runtime_preset(
    "claude-sonnet",
    "anthropic",
    "claude-sonnet-4-5",
    "anthropic_high",
    "claude_200k",
)?;
let alias = preset.provider_alias(anthropic_http_config("api-key"));
assert_eq!(alias.alias, "claude-sonnet");

let grok = model_runtime_preset("grok", "xai", "grok-4.5", "grok", "grok-4.5")?;
let alias = grok.provider_alias(xai_responses_http_config("api-key"));
assert_eq!(alias.model_name, "grok-4.5");
# Ok(())
# }
```

## Provider-neutral cache points

Insert `ContentPart::cache_point()` after a stable user-content block to mark a provider-neutral prompt-cache boundary. Use `ContentPart::cache_point_with_ttl(CachePointTtl::FiveMinutes)` or `OneHour` only for providers such as Anthropic that support per-breakpoint TTLs. The marker is not model-visible content; unsupported provider mappers remove it.

For GPT-5.6 Chat Completions and Responses, Starweaver maps the marker onto the preceding supported block as `prompt_cache_breakpoint: {"mode":"explicit"}`. Configure the request-wide policy with typed `OpenAiPromptCacheOptions`: `mode` is `Implicit` or `Explicit`, and the only supported TTL is `OpenAiPromptCacheTtl::ThirtyMinutes`. A per-point `5m` or `1h` TTL is rejected because GPT-5.6 applies `30m` request-wide. The legacy `prompt_cache_retention` field and GPT-5.6 `prompt_cache_options` are mutually exclusive, including after raw extra-body and metadata overrides are merged into the final HTTP body. Models before GPT-5.6 keep their existing automatic caching behavior: Starweaver filters cache-point markers and rejects typed or raw `prompt_cache_options` rather than sending fields those models reject. Set a stable `prompt_cache_key` for GPT-5.6's reliable implicit and explicit matching. See the [OpenAI prompt caching guide](https://developers.openai.com/api/docs/guides/prompt-caching).

For Anthropic Messages, the marker becomes `cache_control` on the preceding cacheable content block and defaults to `5m`; `1h` is also supported. Existing `anthropic_cache_instructions` and `anthropic_cache_tool_definitions` options cache system and tool-definition prefixes. `anthropic_cache_messages` walks backward to the last cacheable message block, skipping thinking and redacted-thinking blocks, and preserves an existing explicit cache point and its TTL. `anthropic_cache` emits Anthropic's top-level automatic cache control; these two settings are mutually exclusive. Explicit `CachePoint` markers may coexist with automatic caching, but an explicit marker on the final cacheable block must use the same TTL. Starweaver preserves system/tool points and the newest message points while enforcing Anthropic's four-point limit, with automatic caching consuming one slot. Mixed TTL requests are validated so every `1h` point precedes every `5m` point. See the [Claude prompt caching guide](https://platform.claude.com/docs/en/build-with-claude/prompt-caching).

### Anthropic cache usage and pricing

Anthropic reports the aggregate cache creation count in `usage.cache_creation_input_tokens` and may break it down into `usage.cache_creation.ephemeral_5m_input_tokens` and `ephemeral_1h_input_tokens`. Starweaver keeps `Usage.cache_write_tokens` as the reported aggregate across all durations and records the reported one-hour subset in `Usage.cache_write_1h_tokens`. When only the duration breakdown is present, the adapter derives the aggregate from its sum. `Usage.input_tokens` remains inclusive of cache writes and reads.

Raw counters remain provider evidence because public construction and external JSON can be inconsistent. Provider adapters preserve a reported aggregate verbatim and derive it from duration details only when no aggregate is present. Derived accounting should use `Usage::effective_cache_write_tokens`, `effective_cache_write_1h_tokens`, `effective_cache_read_tokens`, and `effective_standard_input_tokens`. These projections repair a one-hour subset larger than its aggregate, cap writes at inclusive input, then cap reads at the remaining input. This write-first policy partitions `Usage.input_tokens` exactly into effective writes, reads, and standard input; Python `Usage` bounds effective inputs to the Rust `u64` domain and exposes the same projections while preserving raw fields in `to_dict()`.

With the `starweaver-usage` `pricing` feature, built-in Anthropic profiles charge default/5-minute writes at 125% of standard input, one-hour writes at 200%, and cache reads at 10%, following Anthropic's published multipliers. A custom pricing profile without a one-hour rate falls back to its default cache-write rate. The aggregate-plus-subset shape is additive for JSON/wire compatibility: a future cache category can retain existing serialized fields. Each new typed Rust subset remains source-breaking for exhaustive struct literals and requires explicit propagation, projection, documentation, and tests.

## OpenAI prompt cache routing

OpenAI prompt caching is automatic for long prompts, but cache hits are still routing-sensitive. Starweaver keeps stable instructions, tool definitions, and append-only conversation history at the front of provider requests. Session-bound cache routing is explicit: the active `ModelConfig` must include `ModelCapability::OpenAiPromptCacheKey`, and the selected OpenAI Responses transport must carry a provider session header. The built-in `gpt5_270k`, `gpt5_350k`, and `gpt5_1m` config presets declare the capability; other presets do not.

For capable Codex OAuth, direct CLI `openai-responses-ws`, and gateway-backed OpenAI Responses transports, Starweaver copies the effective model settings and binds `prompt_cache_key` to the exact provider session-routing value. Codex first canonicalizes unsafe, non-ASCII, or over-64-byte affinity IDs using its stable SHA-256 routing format, then uses that same value in both the session headers and request body. WebSocket requests and HTTP fallback share the normalized headers and body. A plain direct `openai-responses` HTTP transport does not invent a session header and therefore does not receive an automatic session-bound key.

While session-bound routing is active, `AgentContext.session_id` is authoritative. Conflicting case-insensitive `x-session-id` headers, typed cache keys, and raw `prompt_cache_key` body overrides are replaced on owned effective request copies; caller-owned `ModelSettings`, nested header/body maps, and request parameters are not mutated. Without both the explicit capability and an eligible transport, settings remain unchanged. Applications can still configure standalone prompt-cache keys and the legacy `prompt_cache_retention` through typed `OpenAiChatSettings` / `OpenAiResponsesSettings`, raw body settings, or request metadata. GPT-5.6 applications should use typed `prompt_cache_options` instead of `prompt_cache_retention`; Starweaver rejects a final request containing both fields.

`store=false` is independent from prompt caching. It disables provider-side response/conversation storage, not OpenAI's automatic prompt cache. Measure actual prompt-cache reuse with `Usage.cache_read_tokens`, which is populated from OpenAI `cached_tokens` usage details.

## OpenAI GPT-5.6 usage and pricing

OpenAI Chat Completions reports GPT-5.6 cache writes in `usage.prompt_tokens_details.cache_write_tokens`; Responses reports them in `usage.input_tokens_details.cache_write_tokens`. Starweaver normalizes both into `Usage.cache_write_tokens`. Cache reads from `cached_tokens` are normalized into `Usage.cache_read_tokens`, while `Usage.input_tokens` remains the inclusive provider input total.

With the `starweaver-usage` `pricing` feature, the built-in GPT-5.6 Sol, Terra, and Luna profiles charge cache writes at 125% of standard input and cache reads at 10%. Requests at or below 272,000 input tokens use the standard rates. Requests above 272,000 input tokens use the published long-context tier: 2x input, cache-write, and cache-read rates, and 1.5x output rates. Tier selection is per provider request; cumulative estimates should sum the per-request estimates. These catalog values represent standard direct API pricing and do not model Batch, Flex, Priority, regional, promotional, tax, or contract adjustments.

## Provider request audit

`ProtocolModelClient::with_provider_request_audit` records provider HTTP request snapshots outside redacted runtime trace spans. The default SDK path records no provider audit payloads. Applications that need fixture generation, gateway audit, or provider debugging can install an `InMemoryProviderRequestAuditRecorder` or their own `ProviderRequestAuditRecorder` with a `ProviderRequestAuditPolicy`.

Use `ProviderRequestAuditPolicy::metadata_only()` for endpoint and correlation metadata, `redacted_payloads()` for scrubbed headers and JSON bodies, and `full_payloads()` only for explicit local debugging or fixture work.

## Model wrappers

Use `HookedModel` when application code needs a model-level wrapper around any `ModelAdapter`. `ModelExecutionHook` receives typed metadata before the request and after the final response, including model name, provider name, run id, conversation id, streaming flag, and low-cardinality runtime metadata such as `agent_name`.

```rust
use std::sync::Arc;

use async_trait::async_trait;
use starweaver_model::{
    FunctionModel, HookedModel, ModelAdapter, ModelError, ModelExecutionHook,
    ModelExecutionMetadata, ModelMessage, ModelRequestContext, ModelRequestParameters,
    ModelResponse, ModelSettings,
};

struct AuditHook;

#[async_trait]
impl ModelExecutionHook for AuditHook {
    async fn before_model_request(
        &self,
        metadata: ModelExecutionMetadata,
        _messages: &[ModelMessage],
        _settings: Option<&ModelSettings>,
        _params: &ModelRequestParameters,
        _context: &ModelRequestContext,
    ) -> Result<(), ModelError> {
        assert_eq!(metadata.model_name, "function");
        Ok(())
    }
}

let inner = Arc::new(FunctionModel::new(|_, _, _| Ok(ModelResponse::text("ok"))));
let model = HookedModel::new(inner).with_hook(Arc::new(AuditHook));

assert_eq!(ModelAdapter::model_name(&model), "function");
```

## Production request guard

Use the global guard in tests to prevent production HTTP requests:

```rust
use starweaver_model::block_real_model_requests;

let _guard = block_real_model_requests();
assert!(!starweaver_model::allow_real_model_requests());
```

`ProtocolModelClient` checks this guard before calling injected transport, and `ReqwestHttpClient` checks it at the HTTP boundary.
