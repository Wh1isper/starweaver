#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentRunState, FunctionDynamicInstruction, FunctionHistoryProcessor,
    FunctionTool, OutputSchema, StaticCapabilityBundle, TestModel, ToolContext, ToolRegistry,
    ToolResult, UsageLimits,
};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ProtocolFamily,
};

#[derive(Clone)]
struct CaptureModel {
    captured_params: Arc<Mutex<Vec<ModelRequestParameters>>>,
}

#[async_trait]
impl ModelAdapter for CaptureModel {
    fn model_name(&self) -> &'static str {
        "capture"
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
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured_params.lock().unwrap().push(params);
        Ok(ModelResponse::text(r#"{"answer":"ok"}"#))
    }
}

#[tokio::test]
async fn builder_creates_reusable_agent_with_tools() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let tool = FunctionTool::new(
        "echo",
        Some("Echo input".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(tool));

    let agent = AgentBuilder::new(model.clone())
        .instruction("Be concise")
        .output_schema(OutputSchema::new(
            "answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ))
        .usage_limits(UsageLimits::new().with_request_limit(1))
        .history_processor(Arc::new(FunctionHistoryProcessor::new(
            |messages| async move { Ok(messages) },
        )))
        .tool_registry(tools)
        .build();

    let result = agent.run("hello").await.unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    assert_eq!(result.structured_output.unwrap()["answer"], "ok");
    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "echo");
    assert_eq!(params.output_schema.unwrap()["name"], "answer");
}

#[tokio::test]
async fn builder_agents_support_test_model_override() {
    let agent = AgentBuilder::new(Arc::new(TestModel::with_text("production"))).build();

    let overridden = agent
        .override_config()
        .model(Arc::new(TestModel::with_text("test")))
        .build();

    let result = overridden.run("hello").await.unwrap();

    assert_eq!(result.output, "test");
}

#[tokio::test]
async fn builder_applies_capability_bundle() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let tool = FunctionTool::new(
        "bundle_tool",
        Some("Bundle tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let bundle = StaticCapabilityBundle::new("builder-bundle")
        .with_instruction("Use the builder bundle.")
        .with_tool(Arc::new(tool));

    let result = AgentBuilder::new(model.clone())
        .capability_bundle(Arc::new(bundle))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
    let params = model.captured_params.lock().unwrap()[0].clone();
    assert_eq!(params.tools.len(), 1);
    assert_eq!(params.tools[0].name, "bundle_tool");
}

#[tokio::test]
async fn builder_applies_dynamic_instruction() {
    let model = Arc::new(CaptureModel {
        captured_params: Arc::new(Mutex::new(Vec::new())),
    });
    let instruction = FunctionDynamicInstruction::new(|state: AgentRunState| async move {
        Ok(format!("builder dynamic step {}", state.run_step))
    });

    let result = AgentBuilder::new(model)
        .dynamic_instruction(Arc::new(instruction))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, r#"{"answer":"ok"}"#);
}
