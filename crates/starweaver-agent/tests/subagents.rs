#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, FunctionTool, SubagentConfig, SubagentParentTools, SubagentRegistry,
    SubagentTask, SubagentToolInheritancePolicy, TestModel, ToolContext, ToolRegistry, ToolResult,
};
use starweaver_context::{AgentContext, BusMessage};
use starweaver_core::TaskId;
use starweaver_model::{ModelResponse, tool_call_response};
use starweaver_usage::Usage;

fn response_with_usage(text: &str, usage: Usage) -> ModelResponse {
    ModelResponse {
        usage,
        ..ModelResponse::text(text)
    }
}

fn subagent_event<'a>(context: &'a AgentContext, kind: &str) -> &'a starweaver_context::AgentEvent {
    context
        .events
        .events()
        .iter()
        .find(|event| event.kind == kind)
        .unwrap()
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
    assert_eq!(
        subagent_event(&context, "subagent_started").kind,
        "subagent_started"
    );
    assert_eq!(
        subagent_event(&context, "subagent_completed").kind,
        "subagent_completed"
    );
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
    let started = subagent_event(&context, "subagent_started");
    let completed = subagent_event(&context, "subagent_completed");
    assert_eq!(started.payload["task_id"], "task-1");
    assert_eq!(completed.payload["task_id"], "task-1");
    assert_eq!(completed.payload["metadata"]["source"], "test");
}

#[tokio::test]
async fn sdk_subagent_registry_supports_multi_level_nested_delegation() {
    let grandchild =
        Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("grandchild output"))).build());
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_responses(vec![
            tool_call_response(
                "call_nested",
                "delegate",
                serde_json::json!({
                    "subagent_name": "grandchild",
                    "prompt": "nested work",
                    "metadata": {"source": "child"}
                }),
            ),
            ModelResponse::text("child output"),
        ])))
        .build(),
    );
    let registry = Arc::new(
        SubagentRegistry::new()
            .with_subagent(SubagentConfig::new("grandchild", grandchild))
            .with_subagent(
                SubagentConfig::new("child", child).with_tool_inheritance(
                    SubagentToolInheritancePolicy::default()
                        .with_inherit_all_when_empty(true)
                        .with_nested_delegation(true),
                ),
            ),
    );
    let mut context = AgentContext::default();
    context.insert_dependency(SubagentParentTools(
        ToolRegistry::new().with_tool(registry.delegate_tool()),
    ));

    let result = registry
        .delegate("child", "delegate to grandchild", &mut context)
        .await
        .unwrap();

    assert_eq!(result.output, "child output");
    assert!(context.events.events().iter().any(|event| {
        event.kind == "subagent_completed" && event.payload["name"] == "grandchild"
    }));
    assert!(
        context.events.events().iter().any(|event| {
            event.kind == "subagent_completed" && event.payload["name"] == "child"
        })
    );
    assert!(
        context.events.events().iter().any(|event| {
            event.kind == "subagent_stream_record"
                && event.payload["name"] == "child"
                && event.payload["record"]["source"]["agent_name"] == "grandchild"
        }),
        "events: {:#?}",
        context.events.events()
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
                cache_write_tokens: 0,
                cache_read_tokens: 0,
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
        cache_write_tokens: 0,
        cache_read_tokens: 0,
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

#[tokio::test]
async fn sdk_subagent_uses_distinct_agent_id_and_restores_subagent_history() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext {
        run_id: Some(starweaver_core::RunId::from_string("parent-run")),
        ..AgentContext::default()
    };
    context.subagent_history.insert(
        "child-stable".to_string(),
        vec![starweaver_model::ModelMessage::Request(
            starweaver_model::ModelRequest::user_text("previous child prompt"),
        )],
    );
    let task = SubagentTask::new("next child prompt")
        .with_id(TaskId::from_string("task-history"))
        .with_metadata(serde_json::json!({"agent_id": "child-stable"}));

    let envelope = registry
        .delegate_task("child", task, &mut context)
        .await
        .unwrap();

    assert_eq!(envelope.output(), "child output");
    assert_eq!(context.agent_registry["child-stable"].agent_name, "child");
    assert_eq!(
        context.agent_registry["child-stable"]
            .parent_agent_id
            .as_deref(),
        Some("main")
    );
    let history = context.subagent_history.get("child-stable").unwrap();
    assert!(history.len() >= 3);
    assert!(format!("{history:?}").contains("previous child prompt"));
    assert!(format!("{history:?}").contains("next child prompt"));
    assert_eq!(context.build_usage_snapshot().run_id, "parent-run");
}
