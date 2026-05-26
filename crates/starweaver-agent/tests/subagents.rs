#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, SubagentConfig, SubagentRegistry, SubagentTask, TestModel};
use starweaver_context::AgentContext;

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
    assert_eq!(context.events.events()[0].kind, "subagent_complete");
}

#[tokio::test]
async fn sdk_subagent_registry_returns_task_result_envelope() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    let task = SubagentTask::new("hello").with_metadata(serde_json::json!({"task_id": "task-1"}));

    let envelope = registry
        .delegate_task("child", task, &mut context)
        .await
        .unwrap();

    assert_eq!(envelope.name, "child");
    assert_eq!(envelope.task.prompt, "hello");
    assert_eq!(envelope.task.metadata["task_id"], "task-1");
    assert_eq!(envelope.output(), "child output");
    assert_eq!(context.usage.requests, envelope.result.state.usage.requests);
    assert_eq!(
        context.events.events()[0].payload["task"]["task_id"],
        "task-1"
    );
}
