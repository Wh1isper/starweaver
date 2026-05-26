#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use starweaver_context::AgentContext;
use starweaver_model::{
    ModelAdapter, ModelError, ModelMessage, ModelProfile, ModelRequestContext,
    ModelRequestParameters, ModelResponse, ModelResponsePart, ModelSettings, ProtocolFamily,
    ToolCallPart,
};
use starweaver_runtime::{Agent, AgentStreamEvent, OutputSchema};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

#[derive(Clone)]
struct ScriptedModel {
    responses: Arc<Mutex<Vec<ModelResponse>>>,
}

impl ScriptedModel {
    fn new(responses: Vec<ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().rev().collect())),
        }
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
        None
    }

    async fn request(
        &self,
        _messages: Vec<ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
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
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    ToolRegistry::new().with_tool(Arc::new(tool))
}

#[tokio::test]
async fn run_stream_collects_text_run_events() {
    let stream = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "hello",
    )])))
    .run_stream("hi")
    .await
    .unwrap();

    assert_eq!(stream.result.output, "hello");
    assert_eq!(stream.events.len(), 4);
    assert_eq!(stream.events[0].sequence, 0);
    assert!(matches!(
        stream.events[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(matches!(
        stream.events[1].event,
        AgentStreamEvent::ModelRequest { step: 0 }
    ));
    assert!(matches!(
        stream.events[2].event,
        AgentStreamEvent::ModelResponse { step: 1, .. }
    ));
    assert!(
        matches!(stream.events[3].event, AgentStreamEvent::RunComplete { ref output, .. } if output == "hello")
    );
}

#[tokio::test]
async fn stream_events_include_tool_calls_and_returns() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: serde_json::json!({"query": "Paris"}),
            })],
            ..ModelResponse::text("")
        },
        ModelResponse::text("done"),
    ]));

    let stream = Agent::new(model)
        .with_tools(lookup_registry())
        .run_stream("lookup")
        .await
        .unwrap();

    assert!(stream
        .events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::ToolCall { ref call, .. } if call.name == "lookup")));
    assert!(stream
        .events
        .iter()
        .any(|record| matches!(record.event, AgentStreamEvent::ToolReturn { ref tool_return, .. } if tool_return.name == "lookup" && !tool_return.is_error)));
    assert!(matches!(
        stream.events.last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}

#[tokio::test]
async fn stream_events_include_output_retries() {
    let model = Arc::new(ScriptedModel::new(vec![
        ModelResponse::text("not json"),
        ModelResponse::text(r#"{"answer":"ok"}"#),
    ]));

    let stream = Agent::new(model)
        .with_output_schema(OutputSchema::new(
            "answer",
            serde_json::json!({"type": "object", "required": ["answer"]}),
        ))
        .run_stream("answer as json")
        .await
        .unwrap();

    assert_eq!(stream.result.structured_output.unwrap()["answer"], "ok");
    assert!(stream.events.iter().any(|record| matches!(
        record.event,
        AgentStreamEvent::OutputRetry { retries: 1, .. }
    )));
}

#[tokio::test]
async fn run_with_context_can_collect_stream_events() {
    let mut context = AgentContext::default();
    let mut events = Vec::new();

    let result = Agent::new(Arc::new(ScriptedModel::new(vec![ModelResponse::text(
        "context",
    )])))
    .run_with_context_and_stream_events("hi", &mut context, &mut events)
    .await
    .unwrap();

    assert_eq!(result.output, "context");
    assert_eq!(context.events.events().len(), 2);
    assert_eq!(events.len(), 4);
}
