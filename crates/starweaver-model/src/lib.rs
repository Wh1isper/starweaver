//! Provider-neutral model protocol and first wire adapters for Starweaver.

pub mod adapter;
pub mod media;
pub mod message;
pub mod oauth;
pub mod presets;
pub mod profile;
pub mod providers;
pub mod registry;
pub mod request;
pub mod settings;
pub mod stream;
pub mod test;
pub mod transport;
pub mod wrappers;

pub use adapter::{
    allow_real_model_requests, allow_real_model_requests_guard, block_real_model_requests,
    set_allow_real_model_requests, ModelAdapter, ModelError, ModelRequestContext,
    ModelRequestParameters, ModelResponseEventStream, NativeToolDefinition, RealModelRequestGuard,
    ToolDefinition,
};
pub use media::{
    base64_encoded_len, detect_image_dimensions, detect_media_kind, is_document_media_type,
    is_image_media_type, is_video_media_type, parse_data_url, raw_budget_from_base64_limit,
    ImageDimensions, MediaKind, MediaPolicy, MediaPreflight, ParsedDataUrl,
};
pub use message::{
    ContentPart, FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ProviderInfo, ProviderPartInfo, ToolArguments, ToolCallPart, ToolReturnPart,
};
pub use oauth::{
    build_codex_headers, build_codex_model, build_codex_model_with_profile, build_session_headers,
    codex_model_profile, patch_codex_responses_body, CodexOAuthResponsesModel,
    OAuthBearerHttpClient, CODEX_ORIGINATOR,
};
pub use presets::{
    anthropic_http_config, gemini_http_config, get_model_config, get_model_settings,
    google_cloud_http_config, google_cloud_project_http_config, list_model_config_presets,
    list_model_settings_presets, model_runtime_preset, openai_chat_http_config,
    openai_responses_http_config, ModelConfigPreset, ModelConfigPresetData, ModelPresetError,
    ModelRuntimePreset, ModelSettingsPreset,
};
pub use profile::{
    JsonSchemaTransformer, MessageNormalization, ModelProfile, NativeToolKind, ProtocolFamily,
    StructuredOutputMode,
};
pub use providers::client::ProtocolModelClient;
pub use registry::{ProviderAlias, ProviderAliasRegistry};
pub use request::{
    attach_prepared_instructions, context_origin_metadata, prepare_messages, prepare_model_request,
    InstructionPart, OutputMode, PreparedInstruction, PreparedModelRequest,
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_HANDOFF, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, CONTEXT_TYPE_METADATA,
    INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_AGENT, INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION,
    INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_TOOLSET,
};
pub use settings::{
    format_openai_prompt_cache_key, supports_automatic_openai_prompt_cache_key, AnthropicSettings,
    BedrockSettings, CodexSettings, GatewaySettings, GoogleCloudServiceTier, GoogleSettings,
    ModelSettings, OpenAiChatSettings, OpenAiResponsesSettings, ProviderReplaySettings,
    ProviderSettings, ServiceTier, ThinkingSettings, ToolChoice,
};
pub use stream::{
    ModelResponseStreamEvent, ModelStreamState, PartDelta, PartEnd, PartStart, StreamDelta,
    StreamLifecycle,
};
pub use test::{latest_user_text, tool_call_response, FunctionModel, FunctionModelInfo, TestModel};
pub use transport::{
    AuthConfig, DynHttpClient, DynProviderRequestAuditRecorder, DynSleeper, HttpModelConfig,
    HttpRequest, HttpRequestOptions, HttpResponse, InMemoryProviderRequestAuditRecorder,
    MaxTokensParameter, ModelEventStream, ModelHttpClient, ModelSleeper, NoopSleeper,
    ProviderRequestAuditPayloadPolicy, ProviderRequestAuditPolicy, ProviderRequestAuditRecorder,
    ProviderRequestAuditSnapshot, ReqwestHttpClient, RetryPolicy, TokioSleeper,
};
pub use wrappers::{
    ConcurrencyLimitedModel, DynModelAdapter, DynModelExecutionHook, FallbackModel, HookedModel,
    ModelExecutionHook, ModelExecutionMetadata, ProfileOverrideModel,
};
