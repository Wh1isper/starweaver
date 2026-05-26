#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, FunctionTool, SubagentConfig, SubagentRegistry, SubagentTask, TestModel,
    ToolContext, ToolRegistry, ToolResult,
};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_core::{TaskId, Usage};
use starweaver_model::{tool_call_response, ModelResponse};

fn response_with_usage(text: &str, usage: Usage) -> ModelResponse {
    ModelResponse {
        usage,
        ..ModelResponse::text(text)
    }
}

#[test]
fn builder_registers_subagent_config_in_sdk_registry() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child"))).build());
    let builder = AgentBuilder::new(Arc::new(TestModel::with_text("parent")))
        .subagent(SubagentConfig::new("child", child).with_description("Child helper"));

    assert_eq!(builder.subagents().subagents().len(), 1);
    assert_eq!(
        builder
            .subagents()
            .subagent("child")
            .unwrap()
            .description
            .as_deref(),
        Some("Child helper")
    );
}

#[tokio::test]
async fn build_app_keeps_subagents_at_sdk_layer() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let app = AgentBuilder::new(Arc::new(TestModel::with_text("parent output")))
        .subagent(SubagentConfig::new("child", child))
        .build_app();

    let parent = app.run("hello").await.unwrap();
    let mut context = AgentContext::default();
    let child = app
        .subagents()
        .delegate("child", "delegate", &mut context)
        .await
        .unwrap();

    assert_eq!(parent.output, "parent output");
    assert_eq!(child.output, "child output");
}

#[tokio::test]
async fn sdk_subagent_registry_delegates_with_parent_usage() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();

    let result = registry
        .delegate("child", "hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child output");
    assert_eq!(context.usage.requests, result.state.usage.requests);
    assert_eq!(context.events.events()[0].kind, "subagent_started");
    assert_eq!(context.events.events()[1].kind, "subagent_completed");
}

#[tokio::test]
async fn sdk_subagent_registry_returns_task_result_envelope() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    let task = SubagentTask::new("hello")
        .with_id(TaskId::from_string("task-1"))
        .with_metadata(serde_json::json!({"source": "test"}));

    let envelope = registry
        .delegate_task("child", task, &mut context)
        .await
        .unwrap();

    assert_eq!(envelope.name, "child");
    assert_eq!(envelope.task.prompt, "hello");
    assert_eq!(envelope.task.id.as_str(), "task-1");
    assert_eq!(envelope.task.metadata["source"], "test");
    assert_eq!(envelope.output(), "child output");
    assert_eq!(context.usage.requests, envelope.result.state.usage.requests);
    assert_eq!(context.events.events()[0].payload["task_id"], "task-1");
    assert_eq!(context.events.events()[1].payload["task_id"], "task-1");
    assert_eq!(
        context.events.events()[1].payload["metadata"]["source"],
        "test"
    );
}

#[derive(Debug, Eq, PartialEq)]
struct ServiceName(String);

#[tokio::test]
async fn sdk_subagent_child_context_inherits_dependencies_notes_and_usage() {
    let child_model = TestModel::with_responses(vec![
        tool_call_response("call_1", "inspect_context", serde_json::json!({})),
        response_with_usage(
            "child done",
            Usage {
                requests: 1,
                input_tokens: 4,
                output_tokens: 2,
                total_tokens: 6,
                tool_calls: 0,
            },
        ),
    ]);
    let inspect_context = FunctionTool::new(
        "inspect_context",
        Some("Inspect inherited child context".to_string()),
        serde_json::json!({"type": "object"}),
        |ctx: ToolContext, _args| async move {
            let service = ctx.dependency::<ServiceName>().unwrap();
            Ok(ToolResult::new(serde_json::json!({"service": service.0})))
        },
    );
    let child = Arc::new(
        AgentBuilder::new(Arc::new(child_model))
            .tool_registry(ToolRegistry::new().with_tool(Arc::new(inspect_context)))
            .build(),
    );
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    context.insert_dependency(ServiceName("weather".to_string()));
    context.notes.set("lang", "Chinese");
    context.usage = Usage {
        requests: 2,
        input_tokens: 10,
        output_tokens: 4,
        total_tokens: 14,
        tool_calls: 1,
    };

    let result = registry
        .delegate("child", "inspect inherited context", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child done");
    assert_eq!(context.usage.requests, 3);
    assert_eq!(context.usage.tool_calls, 2);
    assert_eq!(context.notes.get("lang"), Some("Chinese"));
    assert!(format!("{:?}", result.all_messages()).contains("weather"));
}

#[tokio::test]
async fn sdk_subagent_child_context_keeps_parent_messages_isolated() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    context.enqueue_message(BusMessage::new(
        "steering",
        serde_json::json!({"text": "parent only"}),
    ));

    let result = registry
        .delegate("child", "hello", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child output");
    assert_eq!(context.messages.len(), 1);
    assert!(context.message_history.is_empty());
}
