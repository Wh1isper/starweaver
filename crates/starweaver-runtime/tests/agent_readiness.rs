#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    adapter::ToolDefinition, ModelAdapter, ModelError, ModelMessage, ModelProfile,
    ModelRequestContext, ModelRequestParameters, ModelResponse, ModelResponsePart, ModelSettings,
    ProtocolFamily, ToolCallPart,
};
use starweaver_runtime::{Agent, AgentCapability, AgentError, CapabilityError, CapabilityResult};

#[derive(Clone)]
struct InspectingModel {
    response: ModelResponse,
    seen_settings: Arc<Mutex<Vec<Option<ModelSettings>>>>,
    seen_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

impl InspectingModel {
    fn new(response: ModelResponse) -> Self {
        Self {
            response,
            seen_settings: Arc::new(Mutex::new(Vec::new())),
            seen_params: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl ModelAdapter for InspectingModel {
    fn model_name(&self) -> &'static str {
        "inspect-model"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("inspect")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: LazyLock<ModelProfile> =
            LazyLock::new(|| ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions));
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.seen_settings.lock().unwrap().push(settings);
        self.seen_params.lock().unwrap().push(params);
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn bare_agent_passes_settings_and_request_params_to_model() {
    let model = Arc::new(InspectingModel::new(ModelResponse::text("ok")));
    let settings = ModelSettings {
        max_tokens: Some(32),
        temperature: Some(0.4),
        ..ModelSettings::default()
    };
    let mut tool_metadata = serde_json::Map::new();
    tool_metadata.insert(
        "starweaver_tool_kind".to_string(),
        serde_json::json!("function"),
    );
    let params = ModelRequestParameters {
        tools: vec![ToolDefinition {
            name: "placeholder".to_string(),
            description: Some("placeholder schema for future tools".to_string()),
            parameters: serde_json::json!({"type": "object"}),
            metadata: tool_metadata,
        }],
        ..ModelRequestParameters::default()
    };

    let result = Agent::new(model.clone())
        .with_model_settings(settings.clone())
        .with_request_params(params.clone())
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    assert_eq!(model.seen_settings.lock().unwrap()[0], Some(settings));
    assert_eq!(model.seen_params.lock().unwrap()[0], params);
}

#[tokio::test]
async fn skip_model_request_capability_bypasses_model_call() {
    let model = Arc::new(InspectingModel::new(ModelResponse::text("from model")));
    let result = Agent::new(model.clone())
        .with_capability(Arc::new(SkipWithResponse))
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "from capability");
    assert_eq!(model.seen_settings.lock().unwrap().len(), 0);
    assert_eq!(result.messages.len(), 2);
}

#[tokio::test]
async fn bare_agent_reports_tool_call_boundary_before_tools_phase() {
    let model = Arc::new(InspectingModel::new(ModelResponse {
        parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
            id: "call_1".to_string(),
            name: "lookup".to_string(),
            arguments: serde_json::json!({"query": "x"}).into(),
        })],
        ..ModelResponse::text("")
    }));

    let error = Agent::new(model).run("call a tool").await.unwrap_err();

    assert!(matches!(error, AgentError::ToolCallsRequireTools));
}

struct SkipWithResponse;

#[async_trait]
impl AgentCapability for SkipWithResponse {
    async fn before_model_request(
        &self,
        _state: &mut starweaver_runtime::AgentRunState,
        _request: &mut starweaver_model::ModelRequest,
        _settings: &mut Option<ModelSettings>,
    ) -> CapabilityResult<()> {
        Err(CapabilityError::SkipModelRequest(Box::new(
            ModelResponse::text("from capability"),
        )))
    }
}
