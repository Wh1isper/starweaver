#![allow(clippy::unwrap_used)]

use serde_json::json;
use starweaver_context::{AgentContext, TASK_SNAPSHOT_EVENT_KIND, TaskContextHandle};
use starweaver_core::{AgentId, ConversationId, RunId};

use super::task_tools;

fn context_with_handle(handle: &TaskContextHandle) -> starweaver_tools::ToolContext {
    let mut dependencies = starweaver_context::DependencyStore::new();
    dependencies.insert(handle.clone());
    starweaver_tools::ToolContext::new(RunId::default(), ConversationId::default(), 0)
        .with_dependencies(dependencies)
}

#[tokio::test]
async fn task_tools_mutate_context_and_emit_snapshots() {
    let mut context = AgentContext::new(AgentId::from_string("agent"));
    let handle = TaskContextHandle::from_context(&context);
    let toolset = task_tools();
    let create = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_create")
        .unwrap();
    let update = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_update")
        .unwrap();
    create
        .call(
            context_with_handle(&handle),
            json!({"subject": "ship", "description": "Ship release"}),
        )
        .await
        .unwrap();
    update
        .call(
            context_with_handle(&handle),
            json!({"task_id": "#1", "status": "in_progress", "active_form": "Shipping"}),
        )
        .await
        .unwrap();

    handle.apply_to(&mut context);
    let tasks = context.tasks();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].subject, "ship");
    assert_eq!(tasks[0].status_str(), "in_progress");
    assert!(
        context
            .events
            .events()
            .iter()
            .any(|event| event.kind == TASK_SNAPSHOT_EVENT_KIND)
    );
}

#[tokio::test]
async fn failed_task_update_still_emits_current_snapshot() {
    let mut context = AgentContext::new(AgentId::from_string("agent"));
    let handle = TaskContextHandle::from_context(&context);
    let toolset = task_tools();
    let create = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_create")
        .unwrap();
    let update = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_update")
        .unwrap();
    create
        .call(
            context_with_handle(&handle),
            json!({"subject": "ship", "description": "Ship release"}),
        )
        .await
        .unwrap();
    handle.apply_to(&mut context);
    let before_events = context.events.len();
    let result = update
        .call(
            context_with_handle(&handle),
            json!({"task_id": "#99", "status": "completed"}),
        )
        .await
        .unwrap();

    assert!(
        result
            .user_content
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap()
            .contains("not found")
    );
    handle.apply_to(&mut context);
    assert_eq!(context.tasks().len(), 1);
    assert!(context.events.len() > before_events);
    assert_eq!(
        context.events.events().last().unwrap().kind,
        TASK_SNAPSHOT_EVENT_KIND
    );
    assert_eq!(
        context.events.events().last().unwrap().payload["tasks"][0]["id"],
        "1"
    );
}

#[tokio::test]
async fn task_update_rejects_unknown_status() {
    let mut context = AgentContext::new(AgentId::from_string("agent"));
    let handle = TaskContextHandle::from_context(&context);
    let toolset = task_tools();
    let create = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_create")
        .unwrap();
    let update = toolset
        .get_tools()
        .into_iter()
        .find(|tool| tool.name() == "task_update")
        .unwrap();
    create
        .call(
            context_with_handle(&handle),
            json!({"subject": "ship", "description": "Ship release"}),
        )
        .await
        .unwrap();

    let error = update
        .call(
            context_with_handle(&handle),
            json!({"task_id": "1", "status": "blocked"}),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("invalid task status"));
    handle.apply_to(&mut context);
    assert_eq!(context.tasks()[0].status_str(), "pending");
}
