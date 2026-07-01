#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponseStreamEvent,
    PartDelta, PartEnd, PartStart, TestModel, ToolCallPart,
};
use starweaver_runtime::{DirectModelRequest, model_request, model_request_stream, tool_call};
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

fn user_message(text: &str) -> ModelMessage {
    ModelMessage::Request(ModelRequest {
        parts: vec![ModelRequestPart::UserPrompt {
            content: vec![starweaver_model::ContentPart::Text {
                text: text.to_string(),
            }],
            name: None,
            metadata: serde_json::Map::new(),
        }],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })
}

#[tokio::test]
async fn direct_model_request_returns_response() {
    let model = TestModel::with_text("direct");
    let response = model_request(
        &model,
        DirectModelRequest::new(vec![user_message("hello")]).with_ids(
            RunId::from_string("run_direct"),
            ConversationId::from_string("conv_direct"),
        ),
    )
    .await
    .unwrap();

    assert_eq!(response.text_output(), "direct");
    assert_eq!(model.captured_messages().len(), 1);
}

#[tokio::test]
async fn direct_model_request_uses_stream_final_events() {
    let model = TestModel::with_stream_events(vec![vec![ModelResponseStreamEvent::FinalResult(
        Box::new(ModelResponse::text("direct streamed")),
    )]]);

    let response = model_request(&model, DirectModelRequest::new(vec![user_message("hello")]))
        .await
        .unwrap();

    assert_eq!(response.text_output(), "direct streamed");
    assert_eq!(model.captured_messages().len(), 1);
}

#[tokio::test]
async fn direct_model_stream_falls_back_to_final_result() {
    let model = TestModel::with_text("stream");
    let events = model_request_stream(&model, DirectModelRequest::new(vec![user_message("hello")]))
        .await
        .unwrap();

    assert_eq!(
        events,
        vec![ModelResponseStreamEvent::FinalResult(Box::new(
            ModelResponse::text("stream")
        ))]
    );
}

#[tokio::test]
async fn direct_tool_call_executes_registry_tool() {
    let tool = FunctionTool::new(
        "echo",
        Some("Echo arguments".to_string()),
        serde_json::json!({"type": "object"}),
        |_ctx: ToolContext, args: serde_json::Value| async move { Ok(ToolResult::new(args)) },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(tool));
    let context = ToolContext::new(
        RunId::from_string("run_tool"),
        ConversationId::from_string("conv_tool"),
        0,
    );
    let call = ToolCallPart {
        id: "call_1".to_string(),
        name: "echo".to_string(),
        arguments: serde_json::json!({"value": 42}).into(),
    };

    let result = tool_call(&tools, context, &call).await;

    assert_eq!(result.name, "echo");
    assert!(!result.is_error);
    assert_eq!(result.content["value"], 42);
}

#[test]
fn model_stream_events_remain_replay_serializable() {
    let events = vec![
        ModelResponseStreamEvent::PartStart(PartStart {
            index: 0,
            part_kind: "text".to_string(),
        }),
        ModelResponseStreamEvent::PartDelta(PartDelta::text(0, "ok")),
        ModelResponseStreamEvent::PartEnd(PartEnd::with_kind(0, "text")),
        ModelResponseStreamEvent::FinalResult(Box::new(ModelResponse::text("ok"))),
    ];

    let encoded = serde_json::to_value(&events).unwrap();
    let decoded: Vec<ModelResponseStreamEvent> = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded, events);
}
