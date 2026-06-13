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
    list_model_config_presets, list_model_settings_presets, model_runtime_preset,
    openai_chat_http_config, openai_responses_http_config, ModelConfigPreset,
    ModelConfigPresetData, ModelPresetError, ModelRuntimePreset, ModelSettingsPreset,
};
pub use profile::{
    JsonSchemaTransformer, MessageNormalization, ModelProfile, NativeToolKind, ProtocolFamily,
    StructuredOutputMode,
};
pub use providers::client::ProtocolModelClient;
pub use registry::{ProviderAlias, ProviderAliasRegistry};
pub use request::{
    prepare_messages, prepare_model_request, InstructionPart, OutputMode, PreparedInstruction,
    PreparedModelRequest,
};
pub use settings::{
    ModelSettings, ProviderReplaySettings, ServiceTier, ThinkingSettings, ToolChoice,
};
pub use stream::{
    ModelResponseStreamEvent, ModelStreamState, PartDelta, PartEnd, PartStart, StreamDelta,
    StreamLifecycle,
};
pub use test::{latest_user_text, tool_call_response, FunctionModel, FunctionModelInfo, TestModel};
pub use transport::{
    AuthConfig, DynHttpClient, DynSleeper, HttpModelConfig, HttpRequest, HttpRequestOptions,
    HttpResponse, MaxTokensParameter, ModelEventStream, ModelHttpClient, ModelSleeper, NoopSleeper,
    ReqwestHttpClient, RetryPolicy, TokioSleeper,
};
pub use wrappers::{ConcurrencyLimitedModel, DynModelAdapter, FallbackModel, ProfileOverrideModel};
