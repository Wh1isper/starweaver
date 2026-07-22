#![allow(clippy::expect_used, clippy::unwrap_used, missing_docs)]

use chrono::{Duration, Utc};
use rusqlite::Connection;
use starweaver_core::{ConversationId, RunId, SessionId};
use starweaver_session::{
    AcquireRunAdmission, AdmitRunControl, DurableRunControlEffect, DurableRunControlStatus,
    LOCAL_SESSION_NAMESPACE, RunRecord, SessionRecord, SessionStore, SessionStoreError,
    deterministic_run_control_operation_id, deterministic_run_control_receipt_id,
};
use starweaver_storage::SqliteStorage;

async fn admitted_run(
    storage: &SqliteStorage,
    session_name: &str,
    run_name: &str,
) -> starweaver_session::RunAdmissionReceipt {
    let store = storage.session_store();
    let session_id = SessionId::from_string(session_name);
    let mut session = SessionRecord::new(session_id.clone());
    session.namespace_id = LOCAL_SESSION_NAMESPACE.to_string();
    store.save_session(session).await.expect("save session");
    store
        .acquire_run_admission(AcquireRunAdmission {
            run: RunRecord::new(
                session_id,
                RunId::from_string(run_name),
                ConversationId::new(),
            ),
            namespace_id: LOCAL_SESSION_NAMESPACE.to_string(),
            host_instance_id: "host-a".to_string(),
            admission_id: format!("admission-{run_name}"),
            lease_expires_at: Utc::now() + Duration::minutes(5),
            idempotency_key: format!("start-{run_name}"),
            command_fingerprint: format!("sha256:start-{run_name}"),
            replaces_waiting_run_id: None,
            hitl_resume_claim_id: None,
        })
        .await
        .expect("acquire run admission")
}

fn steering(
    lease: &starweaver_session::RunAdmissionLease,
    authority: &str,
    key: &str,
    fingerprint: &str,
    text: &str,
) -> AdmitRunControl {
    let operation_id =
        deterministic_run_control_operation_id("steer", authority, &lease.target, key, fingerprint);
    AdmitRunControl {
        lease: lease.clone(),
        authority_binding: authority.to_string(),
        receipt_id: deterministic_run_control_receipt_id(&operation_id),
        operation_id,
        idempotency_key: key.to_string(),
        command_fingerprint: fingerprint.to_string(),
        effect: DurableRunControlEffect::Steer {
            text: text.to_string(),
        },
        created_at: Utc::now(),
    }
}

#[tokio::test]
async fn restart_preserves_exact_control_retry_and_monotonic_delivery_states() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("run-control.sqlite3");
    let storage = SqliteStorage::open(&path).expect("open storage");
    let admission = admitted_run(&storage, "control-session", "control-run").await;
    let command = steering(
        &admission.lease,
        "authority-a",
        "steer-key",
        "sha256:steer-a",
        "change direction",
    );
    let operation_id = command.operation_id.clone();
    let store = storage.session_store();
    let pending = store
        .admit_run_control(command.clone())
        .await
        .expect("admit durable control");
    assert_eq!(pending.status, DurableRunControlStatus::Pending);
    assert_eq!(pending.receipt.state, "pending");
    drop(store);
    drop(storage);

    let reopened = SqliteStorage::open(&path).expect("reopen storage");
    let store = reopened.session_store();
    let loaded = store
        .load_run_control_intent(&admission.lease.target, &operation_id)
        .await
        .expect("load after restart")
        .expect("durable intent");
    assert_eq!(loaded, pending);
    let exact = store
        .admit_run_control(command.clone())
        .await
        .expect("exact admission retry");
    assert_eq!(exact, pending);

    let delivered = store
        .advance_run_control_intent(
            &admission.lease,
            &operation_id,
            DurableRunControlStatus::Pending,
            DurableRunControlStatus::Delivered,
            Utc::now(),
        )
        .await
        .expect("record runtime delivery");
    assert_eq!(delivered.status, DurableRunControlStatus::Delivered);
    assert_eq!(delivered.receipt.state, "delivered");
    let consumed = store
        .advance_run_control_intent(
            &admission.lease,
            &operation_id,
            DurableRunControlStatus::Delivered,
            DurableRunControlStatus::Consumed,
            Utc::now(),
        )
        .await
        .expect("record runtime consumption");
    assert_eq!(consumed.status, DurableRunControlStatus::Consumed);
    assert!(consumed.delivered_at.is_some());
    assert!(consumed.consumed_at.is_some());

    // An exact command retry observes the original consumed operation instead of creating a
    // second inbox item. The receipt and intent remain a one-to-one atomic pair.
    assert_eq!(
        store.admit_run_control(command).await.expect("exact retry"),
        consumed
    );
    let connection = Connection::open(&path).expect("inspect database");
    let receipt_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM run_control_receipts", [], |row| {
            row.get(0)
        })
        .expect("count receipts");
    let intent_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM run_control_intents", [], |row| {
            row.get(0)
        })
        .expect("count intents");
    assert_eq!((receipt_count, intent_count), (1, 1));
}

#[tokio::test]
async fn receipt_and_effect_intent_roll_back_together_across_late_failure_window() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("run-control-rollback.sqlite3");
    let storage = SqliteStorage::open(&path).expect("open storage");
    let admission = admitted_run(&storage, "rollback-session", "rollback-run").await;
    let command = steering(
        &admission.lease,
        "authority-a",
        "rollback-key",
        "sha256:rollback",
        "must be atomic",
    );
    let connection = Connection::open(&path).expect("trigger connection");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_run_control_intent
             BEFORE INSERT ON run_control_intents
             BEGIN SELECT RAISE(ABORT, 'injected late intent failure'); END;",
        )
        .expect("create trigger");
    let error = storage
        .session_store()
        .admit_run_control(command.clone())
        .await
        .expect_err("late insert must fail");
    assert!(matches!(error, SessionStoreError::Failed(_)));
    let receipt_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM run_control_receipts", [], |row| {
            row.get(0)
        })
        .expect("count rolled back receipts");
    let intent_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM run_control_intents", [], |row| {
            row.get(0)
        })
        .expect("count rolled back intents");
    assert_eq!((receipt_count, intent_count), (0, 0));
    connection
        .execute_batch("DROP TRIGGER fail_run_control_intent;")
        .expect("drop trigger");
    storage
        .session_store()
        .admit_run_control(command)
        .await
        .expect("retry after rollback");
}

#[tokio::test]
async fn authority_fingerprint_and_fencing_conflicts_cannot_mutate_control_effect() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let admission = admitted_run(&storage, "fence-session", "fence-run").await;
    let command = steering(
        &admission.lease,
        "authority-a",
        "bound-key",
        "sha256:bound",
        "original",
    );
    let store = storage.session_store();
    let pending = store
        .admit_run_control(command.clone())
        .await
        .expect("admit intent");

    let mut changed_fingerprint = command.clone();
    changed_fingerprint.command_fingerprint = "sha256:different".to_string();
    assert!(matches!(
        store.admit_run_control(changed_fingerprint).await,
        Err(SessionStoreError::IdempotencyConflict(_))
    ));
    let mut changed_authority = command.clone();
    changed_authority.authority_binding = "authority-b".to_string();
    assert!(matches!(
        store.admit_run_control(changed_authority).await,
        Err(SessionStoreError::Conflict(_) | SessionStoreError::IdempotencyConflict(_))
    ));
    let mut stale_lease = admission.lease.clone();
    stale_lease.host_instance_id = "host-stale".to_string();
    stale_lease.fencing_generation += 1;
    assert!(matches!(
        store
            .advance_run_control_intent(
                &stale_lease,
                &pending.operation_id,
                DurableRunControlStatus::Pending,
                DurableRunControlStatus::Delivered,
                Utc::now(),
            )
            .await,
        Err(SessionStoreError::StaleFence(_))
    ));
    assert_eq!(
        store
            .load_run_control_intent(&pending.target, &pending.operation_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        DurableRunControlStatus::Pending
    );

    let expired = store
        .reconcile_expired_run_admissions(
            LOCAL_SESSION_NAMESPACE,
            Utc::now() + Duration::minutes(10),
        )
        .await
        .expect("reconcile expired owner and its effects");
    assert_eq!(expired, vec![pending.target.clone()]);
    let reconciled = store
        .load_run_control_intent(&pending.target, &pending.operation_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reconciled.status, DurableRunControlStatus::Reconciled);
    assert!(reconciled.reconciled_at.is_some());
}
