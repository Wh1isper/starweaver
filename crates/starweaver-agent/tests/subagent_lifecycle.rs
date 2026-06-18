#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{
    AgentBuilder, AgentRuntimePolicy, SubagentConfig, SubagentRegistry, SubagentTask, TestModel,
};
use starweaver_context::AgentContext;
use starweaver_core::{SubagentLifecycleEvent, SubagentLifecycleKind, TaskId};

fn lifecycle_events(context: &AgentContext) -> Vec<&starweaver_context::AgentEvent> {
    context
        .events
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event.kind.as_str(),
                "subagent_started" | "subagent_completed" | "subagent_failed"
            )
        })
        .collect()
}

#[tokio::test]
async fn subagent_lifecycle_events_are_emitted_in_order() {
    let child = Arc::new(AgentBuilder::new(Arc::new(TestModel::with_text("child output"))).build());
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    let task = SubagentTask::new("hello")
        .with_id(TaskId::from_string("task-life"))
        .with_metadata(serde_json::json!({"source": "lifecycle-test"}));

    let result = registry
        .delegate_task("child", task, &mut context)
        .await
        .unwrap();

    let events = lifecycle_events(&context);
    let started: SubagentLifecycleEvent =
        serde_json::from_value(events[0].payload.clone()).unwrap();
    let completed: SubagentLifecycleEvent =
        serde_json::from_value(events[1].payload.clone()).unwrap();

    assert_eq!(result.output(), "child output");
    assert_eq!(events[0].kind, "subagent_started");
    assert_eq!(events[1].kind, "subagent_completed");
    assert_eq!(started.kind, SubagentLifecycleKind::Started);
    assert_eq!(started.name, "child");
    assert_eq!(started.task_id.as_str(), "task-life");
    assert_eq!(started.metadata["source"], "lifecycle-test");
    assert_eq!(completed.kind, SubagentLifecycleKind::Completed);
    assert_eq!(completed.name, "child");
    assert_eq!(completed.task_id.as_str(), "task-life");
    assert_eq!(completed.run_id.unwrap(), result.result.state.run_id);
}

#[tokio::test]
async fn missing_subagent_emits_failed_lifecycle_event() {
    let registry = SubagentRegistry::new();
    let mut context = AgentContext::default();
    let task = SubagentTask::new("hello").with_id(TaskId::from_string("task-missing"));

    let error = registry
        .delegate_task("missing", task, &mut context)
        .await
        .unwrap_err();

    let events = lifecycle_events(&context);
    let event = events[0];
    let failed: SubagentLifecycleEvent = serde_json::from_value(event.payload.clone()).unwrap();

    assert_eq!(event.kind, "subagent_failed");
    assert_eq!(failed.kind, SubagentLifecycleKind::Failed);
    assert_eq!(failed.name, "missing");
    assert_eq!(failed.task_id.as_str(), "task-missing");
    assert_eq!(failed.metadata["error"], "missing_subagent");
    assert!(error.to_string().contains("missing subagent missing"));
}

#[test]
fn subagent_lifecycle_event_serializes_as_core_contract() {
    let event = SubagentLifecycleEvent::new(
        SubagentLifecycleKind::Started,
        "researcher",
        TaskId::from_string("task-1"),
    )
    .with_metadata(serde_json::json!({"source": "unit"}));

    let encoded = serde_json::to_value(&event).unwrap();
    let decoded: SubagentLifecycleEvent = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, event);
    assert_eq!(encoded["kind"], "started");
    assert_eq!(encoded["name"], "researcher");
    assert_eq!(encoded["task_id"], "task-1");
}

#[tokio::test]
async fn failing_subagent_emits_failed_lifecycle_event() {
    let child = Arc::new(
        AgentBuilder::new(Arc::new(TestModel::with_text("never finished")))
            .policy(AgentRuntimePolicy {
                max_steps: 0,
                output_retries: 0,
                ..AgentRuntimePolicy::default()
            })
            .build(),
    );
    let registry = SubagentRegistry::new().with_subagent(SubagentConfig::new("child", child));
    let mut context = AgentContext::default();
    let task = SubagentTask::new("hello").with_id(TaskId::from_string("task-fail"));

    let error = registry
        .delegate_task("child", task, &mut context)
        .await
        .unwrap_err();

    let events = lifecycle_events(&context);
    let started: SubagentLifecycleEvent =
        serde_json::from_value(events[0].payload.clone()).unwrap();
    let failed: SubagentLifecycleEvent = serde_json::from_value(events[1].payload.clone()).unwrap();

    assert_eq!(events[0].kind, "subagent_started");
    assert_eq!(events[1].kind, "subagent_failed");
    assert_eq!(started.kind, SubagentLifecycleKind::Started);
    assert_eq!(failed.kind, SubagentLifecycleKind::Failed);
    assert_eq!(failed.name, "child");
    assert_eq!(failed.task_id.as_str(), "task-fail");
    assert!(failed.run_id.is_some());
    assert!(failed.metadata["error"]
        .as_str()
        .unwrap()
        .contains("step limit exceeded"));
    assert!(error.to_string().contains("step limit exceeded"));
}
