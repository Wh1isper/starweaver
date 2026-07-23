#![allow(clippy::expect_used)]

//! Focused atomicity, idempotency, and validation tests for interaction mutations.

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use serde_json::json;
use starweaver_core::{ConversationId, Metadata, RunId, SessionId};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, ClarificationAnswer, DecideApproval,
    DeferredMutationOutcome, DeferredToolRecord, DurableHostEventClass, DurableHostEventScope,
    ExecutionStatus, InteractionMutationContext, PendingHostEventPublication, ResolveClarification,
    ResolveDeferredTool, RunRecord, SessionRecord, SessionStore, SessionStoreError,
};
use starweaver_storage::SqliteStorage;

fn timestamp(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 21, 14, 0, second)
        .single()
        .expect("timestamp")
}

async fn setup_storage(path: &std::path::Path) -> (SqliteStorage, SessionId, RunId) {
    let storage = SqliteStorage::open(path).expect("storage");
    let session_id = SessionId::from_string("interaction-session");
    let run_id = RunId::from_string("interaction-run");
    let store = storage.session_store();
    store
        .save_session(SessionRecord::new(session_id.clone()))
        .await
        .expect("save session");
    let mut run = RunRecord::new(session_id.clone(), run_id.clone(), ConversationId::new());
    run.sequence_no = 1;
    store.append_run(run).await.expect("append run");
    (storage, session_id, run_id)
}

fn context(
    key: &str,
    fingerprint: &str,
    expected_revision: u64,
    occurred_at: chrono::DateTime<Utc>,
) -> InteractionMutationContext {
    InteractionMutationContext {
        authority_binding: "authority-a".to_string(),
        expected_revision,
        idempotency_key: key.to_string(),
        command_fingerprint: fingerprint.to_string(),
        occurred_at,
        host_event_publication: None,
    }
}

fn decision(occurred_at: chrono::DateTime<Utc>, status: ApprovalStatus) -> ApprovalDecision {
    ApprovalDecision {
        status,
        decided_by: Some("operator-a".to_string()),
        decided_at: occurred_at,
        reason: Some("reviewed".to_string()),
        metadata: Metadata::default(),
    }
}

#[tokio::test]
async fn approval_state_receipt_event_and_exact_replay_are_atomic() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("approval.sqlite3");
    let (storage, session_id, run_id) = setup_storage(&database_path).await;
    let store = storage.session_store();
    store
        .append_approval(ApprovalRecord::new(
            "approval-a",
            session_id.clone(),
            run_id.clone(),
            "action-a",
            "shell",
        ))
        .await
        .expect("append approval");

    let event = PendingHostEventPublication::new(
        "approval-receipt-a",
        0,
        DurableHostEventScope::run(session_id.clone(), run_id.clone()),
        DurableHostEventClass::ApprovalChanged,
        json!({"approval_id": "approval-a", "state": "approved"}),
        timestamp(1),
    )
    .expect("event");
    let mut mutation_context = context("approval-key", "sha256:approval", 1, timestamp(1));
    mutation_context.host_event_publication = Some(event.clone());
    let command = DecideApproval {
        context: mutation_context,
        session_id: session_id.clone(),
        run_id: run_id.clone(),
        approval_id: "approval-a".to_string(),
        decision: decision(timestamp(1), ApprovalStatus::Approved),
    };
    let first = storage
        .decide_approval_atomic(command.clone())
        .expect("decide approval");
    assert_eq!(first.approval.revision, 2);
    assert_eq!(first.approval.status, ApprovalStatus::Approved);
    assert!(!first.receipt.replayed);

    let replay = storage
        .decide_approval_atomic(command)
        .expect("exact replay");
    assert!(replay.receipt.replayed);
    assert_eq!(replay.approval, first.approval);

    let pending = store
        .pending_host_event_publications(10)
        .await
        .expect("pending events");
    assert_eq!(pending, vec![event], "event must be enqueued exactly once");
    let approvals = store
        .load_approvals(&session_id, &run_id)
        .await
        .expect("load approval");
    assert_eq!(
        approvals[0].revision, 2,
        "replay must not increment revision"
    );

    let conflict = storage
        .decide_approval_atomic(DecideApproval {
            context: context("approval-key", "sha256:different", 2, timestamp(2)),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            approval_id: "approval-a".to_string(),
            decision: decision(timestamp(2), ApprovalStatus::Denied),
        })
        .expect_err("key fingerprint conflict");
    assert!(matches!(
        conflict,
        SessionStoreError::IdempotencyConflict(_)
    ));

    let connection = Connection::open(database_path).expect("open database");
    let receipt_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_mutation_receipts",
            [],
            |row| row.get(0),
        )
        .expect("receipt count");
    assert_eq!(receipt_count, 1);
}

#[tokio::test]
async fn outbox_failure_rolls_back_approval_revision_and_receipt() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("rollback.sqlite3");
    let (storage, session_id, run_id) = setup_storage(&database_path).await;
    let store = storage.session_store();
    store
        .append_approval(ApprovalRecord::new(
            "approval-rollback",
            session_id.clone(),
            run_id.clone(),
            "action-a",
            "shell",
        ))
        .await
        .expect("append approval");
    let original = PendingHostEventPublication::new(
        "occupied-publication",
        0,
        DurableHostEventScope::run(session_id.clone(), run_id.clone()),
        DurableHostEventClass::ApprovalChanged,
        json!({"state": "original"}),
        timestamp(1),
    )
    .expect("original event");
    store
        .enqueue_host_event_publications(vec![original])
        .await
        .expect("occupy event key");
    let conflicting = PendingHostEventPublication::new(
        "occupied-publication",
        0,
        DurableHostEventScope::run(session_id.clone(), run_id.clone()),
        DurableHostEventClass::ApprovalChanged,
        json!({"state": "different"}),
        timestamp(1),
    )
    .expect("conflicting event");
    let mut mutation_context = context("rollback-key", "sha256:rollback", 1, timestamp(1));
    mutation_context.host_event_publication = Some(conflicting);
    storage
        .decide_approval_atomic(DecideApproval {
            context: mutation_context,
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            approval_id: "approval-rollback".to_string(),
            decision: decision(timestamp(1), ApprovalStatus::Approved),
        })
        .expect_err("outbox conflict");

    let approvals = store
        .load_approvals(&session_id, &run_id)
        .await
        .expect("load approval");
    assert_eq!(approvals[0].revision, 1);
    assert_eq!(approvals[0].status, ApprovalStatus::Pending);
    assert!(approvals[0].decision.is_none());
    let connection = Connection::open(&database_path).expect("open database");
    let receipt_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_mutation_receipts",
            [],
            |row| row.get(0),
        )
        .expect("receipt count");
    assert_eq!(receipt_count, 0);

    let committed = storage
        .decide_approval_atomic(DecideApproval {
            context: context("rollback-key", "sha256:rollback", 1, timestamp(1)),
            session_id,
            run_id,
            approval_id: "approval-rollback".to_string(),
            decision: decision(timestamp(1), ApprovalStatus::Approved),
        })
        .expect("same key remains available after rollback");
    assert_eq!(committed.approval.revision, 2);
}

#[tokio::test]
async fn deferred_complete_and_fail_are_terminal_revisioned_mutations() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let (storage, session_id, run_id) =
        setup_storage(&tempdir.path().join("deferred.sqlite3")).await;
    let store = storage.session_store();
    for id in ["deferred-complete", "deferred-fail"] {
        let mut record = DeferredToolRecord::new(
            id,
            session_id.clone(),
            run_id.clone(),
            format!("call-{id}"),
            "remote_tool",
        );
        record.status = ExecutionStatus::Waiting;
        store
            .append_deferred_tool(record)
            .await
            .expect("append deferred");
    }

    let completed = storage
        .resolve_deferred_tool_atomic(ResolveDeferredTool {
            context: context("deferred-complete-key", "sha256:complete", 1, timestamp(1)),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            deferred_id: "deferred-complete".to_string(),
            outcome: DeferredMutationOutcome::Completed {
                response: json!({"value": 42}),
                metadata: Metadata::default(),
            },
        })
        .expect("complete deferred");
    assert_eq!(completed.deferred.status, ExecutionStatus::Completed);
    assert_eq!(completed.deferred.revision, 2);
    assert_eq!(completed.receipt.operation, "deferred.complete");

    let failed = storage
        .resolve_deferred_tool_atomic(ResolveDeferredTool {
            context: context("deferred-fail-key", "sha256:fail", 1, timestamp(2)),
            session_id: session_id.clone(),
            run_id: run_id.clone(),
            deferred_id: "deferred-fail".to_string(),
            outcome: DeferredMutationOutcome::Failed {
                response: json!({"code": "unavailable"}),
                metadata: Metadata::default(),
            },
        })
        .expect("fail deferred");
    assert_eq!(failed.deferred.status, ExecutionStatus::Failed);
    assert_eq!(failed.receipt.operation, "deferred.fail");

    let stale = storage
        .resolve_deferred_tool_atomic(ResolveDeferredTool {
            context: context("deferred-stale", "sha256:stale", 1, timestamp(3)),
            session_id,
            run_id,
            deferred_id: "deferred-complete".to_string(),
            outcome: DeferredMutationOutcome::Failed {
                response: json!({"code": "late"}),
                metadata: Metadata::default(),
            },
        })
        .expect_err("stale revision");
    assert!(matches!(stale, SessionStoreError::Conflict(_)));
}

fn clarification_record(id: &str, session_id: &SessionId, run_id: &RunId) -> ApprovalRecord {
    let mut record = ApprovalRecord::new(
        id,
        session_id.clone(),
        run_id.clone(),
        format!("action-{id}"),
        "ask_user_question",
    );
    record.request = json!({
        "questions": [
            {
                "header": "Database",
                "question": "Which database?",
                "multi_select": false,
                "options": [
                    {"label": "PostgreSQL", "description": "Relational"},
                    {"label": "SQLite", "description": "Embedded"}
                ]
            },
            {
                "header": "Notes",
                "question": "Any constraints?",
                "multiSelect": true,
                "options": []
            }
        ]
    });
    record
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn clarification_strictly_validates_durable_request_without_partial_writes() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("clarification.sqlite3");
    let (storage, session_id, run_id) = setup_storage(&database_path).await;
    let store = storage.session_store();
    store
        .append_approval(clarification_record(
            "clarification-a",
            &session_id,
            &run_id,
        ))
        .await
        .expect("append clarification");
    let non_ask = ApprovalRecord::new(
        "not-a-clarification",
        session_id.clone(),
        run_id.clone(),
        "action-normal",
        "shell",
    );
    store
        .append_approval(non_ask)
        .await
        .expect("append non clarification");
    let mut malformed = clarification_record("malformed-request", &session_id, &run_id);
    malformed.request = json!({"questions": "not-an-array"});
    store
        .append_approval(malformed)
        .await
        .expect("append malformed clarification");

    let valid_answers = vec![
        ClarificationAnswer {
            question: "Which database?".to_string(),
            selected_options: vec!["PostgreSQL".to_string()],
            free_text: None,
        },
        ClarificationAnswer {
            question: "Any constraints?".to_string(),
            selected_options: vec![],
            free_text: Some("Must run offline".to_string()),
        },
    ];
    let rejected = vec![
        (
            "missing-answer",
            vec![valid_answers[0].clone()],
            "clarification-a",
        ),
        (
            "unknown-option",
            vec![
                ClarificationAnswer {
                    selected_options: vec!["MySQL".to_string()],
                    ..valid_answers[0].clone()
                },
                valid_answers[1].clone(),
            ],
            "clarification-a",
        ),
        (
            "illegal-multi",
            vec![
                ClarificationAnswer {
                    selected_options: vec!["PostgreSQL".to_string(), "SQLite".to_string()],
                    ..valid_answers[0].clone()
                },
                valid_answers[1].clone(),
            ],
            "clarification-a",
        ),
        (
            "mismatched-question",
            vec![
                ClarificationAnswer {
                    question: "Different question".to_string(),
                    ..valid_answers[0].clone()
                },
                valid_answers[1].clone(),
            ],
            "clarification-a",
        ),
        ("non-ask", valid_answers.clone(), "not-a-clarification"),
        ("malformed", valid_answers.clone(), "malformed-request"),
    ];
    for (key, answers, clarification_id) in rejected {
        storage
            .resolve_clarification_atomic(ResolveClarification {
                context: context(key, &format!("sha256:{key}"), 1, timestamp(1)),
                session_id: session_id.clone(),
                run_id: run_id.clone(),
                clarification_id: clarification_id.to_string(),
                answers,
                response: None,
                resolved_by: Some("operator-a".to_string()),
            })
            .expect_err("invalid clarification must be rejected");
    }

    let records = store
        .load_approvals(&session_id, &run_id)
        .await
        .expect("load approvals");
    for record in &records {
        assert_eq!(record.revision, 1);
        assert_eq!(record.status, ApprovalStatus::Pending);
        assert!(record.decision.is_none());
    }
    let connection = Connection::open(&database_path).expect("open database");
    let receipt_count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM interaction_mutation_receipts",
            [],
            |row| row.get(0),
        )
        .expect("receipt count");
    assert_eq!(
        receipt_count, 0,
        "validation failures must not write receipts"
    );

    let result = storage
        .resolve_clarification_atomic(ResolveClarification {
            context: context("clarification-valid", "sha256:valid", 1, timestamp(2)),
            session_id,
            run_id,
            clarification_id: "clarification-a".to_string(),
            answers: valid_answers.clone(),
            response: Some("Use the selected database".to_string()),
            resolved_by: Some("operator-a".to_string()),
        })
        .expect("resolve clarification");
    assert_eq!(result.clarification.answers, valid_answers);
    assert_eq!(result.clarification.revision, 2);
    assert_eq!(result.approval.status, ApprovalStatus::Approved);
    let metadata = &result
        .approval
        .decision
        .as_ref()
        .expect("decision")
        .metadata;
    assert!(metadata.contains_key("clarification_answers"));
    assert_eq!(
        metadata.get("clarification_response"),
        Some(&json!("Use the selected database"))
    );
}
