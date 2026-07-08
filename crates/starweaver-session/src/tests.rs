#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use serde_json::json;
use starweaver_context::AgentContext;
use starweaver_core::{ConversationId, Metadata, RunId, TaskId, TraceContext};
use starweaver_runtime::{
    AgentCheckpoint, AgentExecutionNode, AgentRunState, AgentStreamEvent, AgentStreamRecord,
};

use super::*;

#[tokio::test]
async fn input_parts_are_stable_json_contracts() {
    let input = vec![
        InputPart::text("hello"),
        InputPart::url("https://example.com"),
        InputPart::command("plan", vec!["--fast".to_string()]),
    ];

    let value = serde_json::to_value(&input).unwrap();
    assert_eq!(value[0]["kind"], "text");
    assert_eq!(value[1]["kind"], "url");
    assert_eq!(value[2]["kind"], "command");
    assert_eq!(
        serde_json::from_value::<Vec<InputPart>>(value).unwrap(),
        input
    );
}

#[test]
fn deferred_tool_facades_round_trip_records_and_decisions() {
    let session_id = SessionId::from_string("session-deferred");
    let run_id = RunId::from_string("run-deferred");
    let mut record =
        DeferredToolRecord::new("deferred-1", session_id, run_id, "call-1", "slow_tool");
    record.request = json!({"query":"rust"});
    let requests = DeferredToolRequests::from_records(&[record.clone()]);
    assert_eq!(requests.requests[0].arguments["query"], "rust");
    let rebuilt = requests.requests[0].clone().into_record();
    assert_eq!(rebuilt.deferred_id, "deferred-1");
    assert_eq!(rebuilt.request["query"], "rust");

    let mut result_metadata = Metadata::default();
    result_metadata.insert("source".to_string(), json!("worker"));
    let mut result = DeferredToolResult::completed("deferred-1", json!({"answer":"ok"}));
    result.metadata = result_metadata;
    let results = DeferredToolResults::new([result.clone()]);
    assert_eq!(results.results.len(), 1);
    result.apply_to_record(&mut record);
    assert_eq!(record.status, ExecutionStatus::Completed);
    assert_eq!(record.response["answer"], "ok");
    assert_eq!(record.metadata["source"], "worker");

    let decision = ToolApprovalDecision::approved()
        .with_override_arguments(json!({"path":"safe.txt"}))
        .into_approval_decision();
    assert_eq!(decision.status, ApprovalStatus::Approved);
    assert_eq!(decision.metadata["override_arguments"]["path"], "safe.txt");

    let denied = ToolApprovalDecision::denied("unsafe").into_approval_decision();
    assert_eq!(denied.status, ApprovalStatus::Denied);
    assert_eq!(denied.reason.as_deref(), Some("unsafe"));
}

#[test]
fn hitl_records_are_derived_from_tool_return_metadata() {
    let session_id = SessionId::from_string("session-hitl");
    let run_id = RunId::from_string("run-hitl");
    let trace_context = TraceContext::from_trace_id("trace-hitl");
    let mut approval_metadata = Metadata::default();
    approval_metadata.insert("control_flow".to_string(), json!("approval_required"));
    approval_metadata.insert("approval".to_string(), json!({"command":"rm -rf target"}));

    let approval_input = ToolReturnRecordInput::new(
        &session_id,
        &run_id,
        "call-approval",
        "shell",
        &approval_metadata,
    )
    .with_trace_context(&trace_context)
    .with_policy(json!("defer"));
    let approval = ApprovalRecord::from_tool_return(&approval_input).unwrap();
    assert_eq!(approval.approval_id, "approval_run-hitl_call-approval");
    assert_eq!(approval.action_name, "shell");
    assert_eq!(approval.request["command"], "rm -rf target");
    assert_eq!(approval.status, ApprovalStatus::Pending);
    assert_eq!(approval.metadata["policy"], "defer");
    assert_eq!(
        approval.trace_context.trace_id.as_deref(),
        Some("trace-hitl")
    );

    let mut deferred_metadata = Metadata::default();
    deferred_metadata.insert("control_flow".to_string(), json!("call_deferred"));
    deferred_metadata.insert("deferred".to_string(), json!({"url":"https://example.com"}));
    let deferred_input = ToolReturnRecordInput::new(
        &session_id,
        &run_id,
        "call-deferred",
        "fetch",
        &deferred_metadata,
    )
    .with_policy(json!("prompt"));
    let deferred = DeferredToolRecord::from_tool_return(&deferred_input).unwrap();
    assert_eq!(deferred.deferred_id, "deferred_run-hitl_call-deferred");
    assert_eq!(deferred.tool_name, "fetch");
    assert_eq!(deferred.request["url"], "https://example.com");
    assert_eq!(deferred.status, ExecutionStatus::Waiting);
    assert_eq!(deferred.metadata["policy"], "prompt");

    let ignored_metadata = Metadata::default();
    let ignored = ToolReturnRecordInput::new(
        &session_id,
        &run_id,
        "call-normal",
        "read",
        &ignored_metadata,
    );
    assert!(ApprovalRecord::from_tool_return(&ignored).is_none());
    assert!(DeferredToolRecord::from_tool_return(&ignored).is_none());
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn in_memory_store_saves_session_runs_and_resume_snapshot() {
    let store = InMemorySessionStore::new();
    let session_id = SessionId::from_string("session-1");
    let run_id = RunId::from_string("run-1");
    let conversation_id = ConversationId::from_string("conv-1");
    let mut session = SessionRecord::new(session_id.clone());
    session.profile = Some("default".to_string());
    session.workspace = Some("workspace".to_string());
    session.state = AgentContext::default().export_state();
    session.trace_context = TraceContext::from_trace_id("trace-1");
    store.save_session(session).await.unwrap();

    let mut run = RunRecord::new(session_id.clone(), run_id.clone(), conversation_id.clone());
    run.input = vec![InputPart::text("hello")];
    run.trace_context = TraceContext::from_trace_id("trace-run");
    run.parent_run_id = Some(RunId::from_string("run-parent"));
    run.parent_task_id = Some(TaskId::from_string("task-parent"));
    store.append_run(run).await.unwrap();
    store
        .update_run_status(&session_id, &run_id, RunStatus::Running, None)
        .await
        .unwrap();

    let mut run_state = AgentRunState::new(run_id.clone(), conversation_id);
    run_state.run_step = 1;
    let checkpoint =
        AgentCheckpoint::new(AgentExecutionNode::ModelResponse, &run_state).with_stream_cursor(0);
    let checkpoint_id = checkpoint.checkpoint_id.clone();
    store
        .append_checkpoint(&session_id, checkpoint)
        .await
        .unwrap();
    store
        .append_stream_records(
            &session_id,
            &run_id,
            vec![
                AgentStreamRecord::new(0, AgentStreamEvent::ModelRequest { step: 0 }),
                AgentStreamRecord::new(
                    1,
                    AgentStreamEvent::RunComplete {
                        run_id: run_id.clone(),
                        output: "ok".to_string(),
                    },
                ),
            ],
        )
        .await
        .unwrap();
    store
        .append_approval(ApprovalRecord::new(
            "approval-1",
            session_id.clone(),
            run_id.clone(),
            "call-1",
            "shell",
        ))
        .await
        .unwrap();
    store
        .append_deferred_tool(DeferredToolRecord::new(
            "deferred-1",
            session_id.clone(),
            run_id.clone(),
            "call-2",
            "search",
        ))
        .await
        .unwrap();
    store
        .save_stream_cursor(
            &session_id,
            &run_id,
            StreamCursorRef::new("display", "run:run-1", 7),
        )
        .await
        .unwrap();

    let snapshot = store.resume_snapshot(&session_id, &run_id).await.unwrap();
    let trace = store.compact_run_trace(&session_id, &run_id).await.unwrap();
    let session_trace = store.compact_session_trace(&session_id).await.unwrap();

    assert_eq!(
        snapshot.latest_checkpoint.unwrap().checkpoint_id,
        checkpoint_id.clone()
    );
    assert_eq!(snapshot.stream_records.len(), 1);
    assert_eq!(snapshot.stream_records[0].sequence, 1);
    assert_eq!(snapshot.approvals.len(), 1);
    assert_eq!(snapshot.deferred_tools.len(), 1);
    assert_eq!(
        snapshot.run.parent_run_id.as_ref().map(RunId::as_str),
        Some("run-parent")
    );
    assert_eq!(
        snapshot.run.parent_task_id.as_ref().map(TaskId::as_str),
        Some("task-parent")
    );
    assert!(
        snapshot
            .stream_cursors
            .iter()
            .any(|cursor| cursor.family == "display")
    );
    assert_eq!(trace.checkpoints, vec![checkpoint_id]);
    assert_eq!(trace.approvals, 1);
    assert_eq!(trace.deferred_tools, 1);
    assert_eq!(trace.stream_cursor, Some(1));
    assert_eq!(trace.trace_context.trace_id.as_deref(), Some("trace-run"));
    assert_eq!(
        trace.parent_run_id.as_ref().map(RunId::as_str),
        Some("run-parent")
    );
    assert_eq!(
        trace.parent_task_id.as_ref().map(TaskId::as_str),
        Some("task-parent")
    );
    assert_eq!(session_trace.runs, 1);
    assert_eq!(session_trace.profile.as_deref(), Some("default"));
}

#[tokio::test]
async fn list_sessions_filters_and_orders_by_update_time() {
    let store = InMemorySessionStore::new();
    let mut first = SessionRecord::new(SessionId::from_string("session-a"));
    first.profile = Some("default".to_string());
    first.workspace = Some("repo-a".to_string());
    let mut second = SessionRecord::new(SessionId::from_string("session-b"));
    second.profile = Some("research".to_string());
    second.workspace = Some("repo-a".to_string());
    store.save_session(first).await.unwrap();
    store.save_session(second).await.unwrap();

    let listed = store
        .list_sessions(SessionFilter {
            workspace: Some("repo-a".to_string()),
            limit: Some(1),
            ..SessionFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].session_id.as_str(), "session-b");

    let filtered = store
        .list_sessions(SessionFilter {
            profile: Some("default".to_string()),
            ..SessionFilter::default()
        })
        .await
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].session_id.as_str(), "session-a");
}

#[tokio::test]
async fn append_stream_records_is_idempotent_by_sequence() {
    let store = InMemorySessionStore::new();
    let session_id = SessionId::from_string("session-stream");
    let run_id = RunId::from_string("run-stream");
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .unwrap();
    store
        .append_run(RunRecord::new(
            session_id.clone(),
            run_id.clone(),
            ConversationId::from_string("conv-stream"),
        ))
        .await
        .unwrap();
    store
        .append_stream_records(
            &session_id,
            &run_id,
            vec![
                AgentStreamRecord::new(1, AgentStreamEvent::ModelRequest { step: 1 }),
                AgentStreamRecord::new(0, AgentStreamEvent::ModelRequest { step: 0 }),
                AgentStreamRecord::new(1, AgentStreamEvent::ModelRequest { step: 1 }),
            ],
        )
        .await
        .unwrap();

    let replay = store
        .replay_stream_records_after(&session_id, &run_id, Some(0))
        .await
        .unwrap();
    assert_eq!(replay.len(), 1);
    assert_eq!(replay[0].sequence, 1);
}

#[tokio::test]
async fn in_memory_store_rejects_orphan_child_records() {
    let store = InMemorySessionStore::new();
    let session_id = SessionId::from_string("missing-session");
    let run_id = RunId::from_string("missing-run");
    let run = RunRecord::new(
        session_id.clone(),
        run_id.clone(),
        ConversationId::from_string("missing-conv"),
    );
    assert!(matches!(
        store.append_run(run).await,
        Err(SessionStoreError::NotFound(_))
    ));
    assert!(matches!(
        store
            .append_stream_records(
                &session_id,
                &run_id,
                vec![AgentStreamRecord::new(
                    0,
                    AgentStreamEvent::ModelRequest { step: 0 },
                )],
            )
            .await,
        Err(SessionStoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn records_round_trip_through_json() {
    let session_id = SessionId::from_string("session-json");
    let run_id = RunId::from_string("run-json");
    let mut run = RunRecord::new(session_id, run_id, ConversationId::from_string("conv-json"));
    run.input = vec![InputPart::text("hello")];
    run.structured_output = json!({"ok": true});
    run.parent_run_id = Some(RunId::from_string("run-parent-json"));
    run.parent_task_id = Some(TaskId::from_string("task-parent-json"));
    let mut metadata = Metadata::default();
    metadata.insert("source".to_string(), json!("test"));
    run.metadata = metadata;

    let value = serde_json::to_value(&run).unwrap();
    assert_eq!(value["status"], "queued");
    assert_eq!(value["parent_run_id"], "run-parent-json");
    assert_eq!(value["parent_task_id"], "task-parent-json");
    let decoded = serde_json::from_value::<RunRecord>(value).unwrap();
    assert_eq!(decoded, run);
}

#[tokio::test]
async fn session_store_executor_persists_runtime_checkpoints() {
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = SessionId::from_string("session-executor");
    let session = SessionRecord::new(session_id.clone());
    store.save_session(session).await.unwrap();
    let executor = Arc::new(SessionStoreExecutor::new(store.clone(), session_id.clone()));
    let run_id = RunId::from_string("run-executor");
    let conversation_id = ConversationId::from_string("conv-executor");
    store
        .append_run(RunRecord::new(
            session_id.clone(),
            run_id.clone(),
            conversation_id.clone(),
        ))
        .await
        .unwrap();

    let mut state = AgentRunState::new(run_id.clone(), conversation_id);
    state.run_step = 2;
    starweaver_runtime::AgentExecutor::checkpoint(
        executor.as_ref(),
        AgentCheckpoint::new(AgentExecutionNode::ToolReturn, &state),
    )
    .await
    .unwrap();

    assert_eq!(
        store
            .compact_run_trace(&session_id, &run_id)
            .await
            .unwrap()
            .checkpoints
            .len(),
        1
    );
}
