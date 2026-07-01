#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_context::AgentContext;
use starweaver_model::{ModelResponse, TestModel, tool_call_response};
use starweaver_runtime::Agent;
use starweaver_tools::{FunctionTool, ToolContext, ToolRegistry, ToolResult};

#[derive(Debug, Eq, PartialEq)]
struct WeatherService {
    city: String,
}

#[tokio::test]
async fn runtime_passes_typed_dependencies_to_tools() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "weather", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let weather_tool = FunctionTool::new(
        "weather",
        Some("Return weather city".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let service = ctx.dependency::<WeatherService>().unwrap();
            Ok(ToolResult::new(serde_json::json!({"city": service.city})))
        },
    );
    let tools = ToolRegistry::new().with_tool(Arc::new(weather_tool));
    let mut context = AgentContext::default();
    context.insert_dependency(WeatherService {
        city: "Paris".to_string(),
    });

    let result = Agent::new(Arc::new(model))
        .with_tools(tools)
        .run_with_context("weather", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(format!("{:?}", result.all_messages()).contains("Paris"));
}

#[tokio::test]
async fn runtime_passes_named_dependencies_to_tools() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "answer", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let tool = FunctionTool::new(
        "answer",
        Some("Return named dependency".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let answer = ctx.named_dependency::<u32>("answer").unwrap();
            Ok(ToolResult::new(serde_json::json!({"answer": *answer})))
        },
    );
    let mut context = AgentContext::default();
    context.insert_named_dependency("answer", 42_u32);

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("answer", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    assert!(format!("{:?}", result.all_messages()).contains("42"));
}

#[tokio::test]
async fn runtime_passes_state_and_notes_to_tools() {
    let model = TestModel::with_responses(vec![
        tool_call_response("call_1", "context_snapshot", serde_json::json!({})),
        ModelResponse::text("done"),
    ]);
    let tool = FunctionTool::new(
        "context_snapshot",
        Some("Return context state and note snapshots".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let agent_context = ctx.dependency::<AgentContext>().unwrap();
            Ok(ToolResult::new(serde_json::json!({
                "workspace": agent_context.state.get("workspace").unwrap()["root"],
                "language": agent_context.notes.get("language").unwrap(),
            })))
        },
    );
    let mut context = AgentContext::default();
    context
        .state
        .set("workspace", serde_json::json!({"root": "/repo"}));
    context.notes.set("language", "Chinese");

    let result = Agent::new(Arc::new(model))
        .with_tools(ToolRegistry::new().with_tool(Arc::new(tool)))
        .run_with_context("context", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "done");
    let messages = format!("{:?}", result.all_messages());
    assert!(messages.contains("/repo"));
    assert!(messages.contains("Chinese"));
}
