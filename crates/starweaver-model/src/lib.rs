//! Provider-neutral model protocol and first wire adapters for Starweaver.

pub mod adapter;
pub mod message;
pub mod presets;
pub mod profile;
pub mod providers;
pub mod registry;
pub mod settings;
pub mod stream;
pub mod test;
pub mod transport;

pub use adapter::{
    allow_real_model_requests, allow_real_model_requests_guard, block_real_model_requests,
    set_allow_real_model_requests, ModelAdapter, ModelError, ModelRequestContext,
    ModelRequestParameters, NativeToolDefinition, RealModelRequestGuard, ToolDefinition,
};
pub use message::{
    ContentPart, FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ModelResponsePart, ProviderInfo, ToolCallPart, ToolReturnPart,
};
pub use presets::{
    anthropic_http_config, gemini_http_config, get_model_config, get_model_settings,
    list_model_config_presets, list_model_settings_presets, model_runtime_preset,
    openai_chat_http_config, openai_responses_http_config, ModelConfigPreset,
    ModelConfigPresetData, ModelPresetError, ModelRuntimePreset, ModelSettingsPreset,
};
pub use profile::{MessageNormalization, ModelProfile, ProtocolFamily, StructuredOutputMode};
pub use providers::client::ProtocolModelClient;
pub use registry::{ProviderAlias, ProviderAliasRegistry};
pub use settings::{ModelSettings, ServiceTier, ThinkingSettings, ToolChoice};
pub use stream::{ModelResponseStreamEvent, PartDelta, PartEnd, PartStart};
pub use test::{latest_user_text, tool_call_response, FunctionModel, FunctionModelInfo, TestModel};
pub use transport::{
    AuthConfig, DynHttpClient, DynSleeper, HttpModelConfig, HttpRequest, HttpRequestOptions,
    HttpResponse, ModelHttpClient, ModelSleeper, NoopSleeper, ReqwestHttpClient, RetryPolicy,
    TokioSleeper,
};
