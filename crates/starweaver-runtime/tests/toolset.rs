#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ProtocolFamily,
};
use starweaver_runtime::Agent;
use starweaver_tools::{
    DynToolset, FunctionTool, StaticToolset, ToolContext, ToolInstruction, ToolRegistry, ToolResult,
};

#[derive(Clone, Default)]
struct CaptureModel {
    captured: Arc<Mutex<Vec<Vec<ModelMessage>>>>,
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
        static PROFILE: LazyLock<ModelProfile> =
            LazyLock::new(|| ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions));
        &PROFILE
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        None
    }

    async fn request(
        &self,
        messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(messages);
        self.captured_params.lock().unwrap().push(params);
        Ok(ModelResponse::text("ok"))
    }
}

#[tokio::test]
async fn toolset_instructions_are_injected_on_first_request() {
    let model = Arc::new(CaptureModel::default());
    let tool = FunctionTool::new(
        "echo",
        Some("Echo".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let toolset = StaticToolset::new("echo-set")
        .with_tool(Arc::new(tool))
        .with_instruction(ToolInstruction::new("echo-set", "Use echo for mirroring."));
    let toolset: DynToolset = Arc::new(toolset);
    let registry = ToolRegistry::new().with_toolset(&toolset);

    Agent::new(model.clone())
        .with_instruction("Base instruction.")
        .with_tools(registry)
        .run("hello")
        .await
        .unwrap();

    let captured = model.captured.lock().unwrap()[0].clone();
    assert!(format!("{captured:?}").contains("Base instruction"));
    let captured_params = model.captured_params.lock().unwrap()[0].clone();
    assert!(captured_params
        .instructions
        .iter()
        .any(
            |instruction| instruction.text.contains("Use echo for mirroring")
                && !instruction.dynamic
        ));
}
