#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, LazyLock, Mutex};

use async_trait::async_trait;
use starweaver_agent::{
    AgentBuilder, AgentRunOptions, FunctionTool, TestModel, ToolContext, ToolResult,
};
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelSettings, ModelSettings as CapturedSettings,
    ProtocolFamily,
};

#[derive(Clone, Default)]
struct CapturedRequest {
    messages: Vec<ModelMessage>,
    settings: Option<CapturedSettings>,
    params: ModelRequestParameters,
}

#[derive(Clone)]
struct CaptureModel {
    captured: Arc<Mutex<Vec<CapturedRequest>>>,
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
        settings: Option<ModelSettings>,
        params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        self.captured.lock().unwrap().push(CapturedRequest {
            messages,
            settings,
            params,
        });
        Ok(ModelResponse::text("ok"))
    }
}

#[tokio::test]
async fn session_run_options_add_toolsets_settings_params_and_instructions_for_one_run() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CaptureModel {
        captured: captured.clone(),
    });
    let run_tool = Arc::new(FunctionTool::new(
        "run_tool",
        Some("Run-only tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let mut session = AgentBuilder::new(model).build_app().session();
    let mut params = ModelRequestParameters::default();
    params
        .extra_body
        .insert("route".to_string(), serde_json::json!("run"));

    let result = session
        .run_with_options(
            "hello",
            AgentRunOptions::new()
                .instruction("run-only instruction")
                .model_settings(ModelSettings {
                    temperature: Some(0.2),
                    ..ModelSettings::default()
                })
                .request_params(params)
                .tool(run_tool),
        )
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
    let captured_snapshot = captured.lock().unwrap().clone();
    assert_eq!(captured_snapshot.len(), 1);
    assert_eq!(captured_snapshot[0].params.tools.len(), 1);
    assert_eq!(captured_snapshot[0].params.tools[0].name, "run_tool");
    assert_eq!(captured_snapshot[0].params.extra_body["route"], "run");
    assert_eq!(
        captured_snapshot[0].settings.as_ref().unwrap().temperature,
        Some(0.2)
    );
    assert!(format!("{:?}", captured_snapshot[0].messages).contains("run-only instruction"));
}

#[tokio::test]
async fn session_run_options_can_replace_base_tools_for_one_run() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CaptureModel {
        captured: captured.clone(),
    });
    let base_tool = Arc::new(FunctionTool::new(
        "base_tool",
        Some("Base tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let run_tool = Arc::new(FunctionTool::new(
        "run_tool",
        Some("Run-only tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let mut session = AgentBuilder::new(model)
        .tool(base_tool)
        .build_app()
        .session();

    session
        .run_with_options(
            "hello",
            AgentRunOptions::new().tool(run_tool).replace_tools(),
        )
        .await
        .unwrap();

    let tool_names = captured.lock().unwrap()[0]
        .params
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(tool_names, vec!["run_tool".to_string()]);
}

#[tokio::test]
async fn session_run_options_do_not_mutate_reusable_session_agent() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let model = Arc::new(CaptureModel {
        captured: captured.clone(),
    });
    let run_tool = Arc::new(FunctionTool::new(
        "run_tool",
        Some("Run-only tool".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    ));
    let mut session = AgentBuilder::new(model).build_app().session();

    session
        .run_with_options("first", AgentRunOptions::new().tool(run_tool))
        .await
        .unwrap();
    session.run("second").await.unwrap();

    let tool_counts = captured
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.params.tools.len())
        .collect::<Vec<_>>();
    assert_eq!(tool_counts, vec![1, 0]);
}

#[tokio::test]
async fn session_run_iter_accepts_run_options() {
    let mut session = AgentBuilder::new(Arc::new(TestModel::with_text("iter")))
        .build_app()
        .session();

    let result = session
        .run_iter_with_options(
            "hello",
            AgentRunOptions::new().instruction("inspect iterations"),
        )
        .await
        .unwrap();

    assert_eq!(result.result.output, "iter");
    assert!(!result.iterations.steps().is_empty());
}
