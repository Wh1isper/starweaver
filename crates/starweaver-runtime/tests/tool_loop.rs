#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelResponsePart, ModelSettings, ProtocolFamily,
    ToolCallPart,
};
use starweaver_runtime::Agent;
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
    captured_settings: Arc<Mutex<Vec<Option<ModelSettings>>>>,
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
    defaults: Option<ModelSettings>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
            captured: Arc::new(Mutex::new(Vec::new())),
            captured_settings: Arc::new(Mutex::new(Vec::new())),
            captured_params: Arc::new(Mutex::new(Vec::new())),
            defaults: None,
        }
    }

    fn with_defaults(mut self, defaults: ModelSettings) -> Self {
        self.defaults = Some(defaults);
        self
    }
}

#[async_trait]
impl ModelAdapter for ScriptedModel {
    fn model_name(&self) -> &'static str {
        "scripted"
    }

    fn provider_name(&self) -> Option<&'static str> {
        Some("test")
    }

    fn profile(&self) -> &ModelProfile {
        static PROFILE: ModelProfile =
            ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        self.defaults.as_ref()
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(messages);
        self.captured_settings.lock().unwrap().push(settings);
        self.captured_params.lock().unwrap().push(params);
        self.responses
            .lock()
            .unwrap()
            .pop()
            .ok_or_else(|| ModelError::Transport("script exhausted".to_string()))
    }
}

fn lookup_registry() -> ToolRegistry {
    let tool = FunctionTool::new(
        "lookup",
        Some("Lookup a value".to_string()),
        serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        |_ctx: ToolContext, args: serde_json::Value| async move {
            Ok(ToolResult::new(serde_json::json!({
                "value": args["query"].as_str().unwrap_or_default()
            })))
        },
    );
    ToolRegistry::new().with_tool(Arc::new(tool))
}

#[tokio::test]
async fn agent_executes_tool_calls_and_continues_model_loop() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({"query": "Paris"}).into(),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("Paris result"),
    ]));

    let result = Agent::new(model.clone())
        .with_tools(lookup_registry())
        .run("lookup Paris")
        .await
        .unwrap();

    assert_eq!(result.output, "Paris result");
    assert_eq!(result.messages.len(), 4);
    assert_eq!(result.new_messages().len(), 4);
    let second_request_history = model.captured.lock().unwrap()[1].clone();
    let second_request = second_request_history.last().unwrap();
    assert!(format!("{second_request:?}").contains("ToolReturn"));
    assert!(format!("{second_request:?}").contains("Paris"));
}

#[tokio::test]
async fn agent_continues_with_prior_message_history() {
    let first_model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("first")]));
    let first = Agent::new(first_model).run("first prompt").await.unwrap();

    let second_model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("second")]));
    let second = Agent::new(second_model.clone())
        .run_with_history("second prompt", first.new_messages().to_vec())
        .await
        .unwrap();

    assert_eq!(second.output, "second");
    assert_eq!(second.history_len, 2);
    assert_eq!(second.new_messages().len(), 2);
    assert_eq!(second.all_messages().len(), 4);
    let captured = second_model.captured.lock().unwrap()[0].clone();
    assert_eq!(captured.len(), 3);
}

#[tokio::test]
async fn agent_merges_model_default_settings_with_agent_settings() {
    let defaults = ModelSettings {
        max_tokens: Some(128),
        temperature: Some(0.1),
        ..ModelSettings::default()
    };
    let model =
        Arc::new(ScriptedModel::new(vec![ModelResponse::text("ok")]).with_defaults(defaults));

    Agent::new(model.clone())
        .with_model_settings(ModelSettings {
            temperature: Some(0.7),
            ..ModelSettings::default()
        })
        .run("settings")
        .await
        .unwrap();

    let settings = model.captured_settings.lock().unwrap()[0].clone().unwrap();
    assert_eq!(settings.max_tokens, Some(128));
    assert_eq!(settings.temperature, Some(0.7));
}

#[tokio::test]
async fn agent_passes_registered_tool_definitions_to_model() {
    let model = Arc::new(ScriptedModel::new(vec![ModelResponse::text("ok")]));

    Agent::new(model.clone())
        .with_tools(lookup_registry())
        .run("what tools exist")
        .await
        .unwrap();

    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "lookup");
}
