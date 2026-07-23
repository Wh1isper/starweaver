#![allow(clippy::expect_used)]

//! Focused `SQLite` model-selection durability and atomicity tests.

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use starweaver_core::from_versioned_json;
use starweaver_session::{
    DurableHostEventClass, DurableHostEventScope, InitializeModelSelection,
    ModelSelectionMutationReceipt, PendingHostEventPublication, SelectModel, SessionStore,
    SessionStoreError,
};
use starweaver_storage::SqliteStorage;

fn timestamp(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, second)
        .single()
        .expect("timestamp")
}

fn initialization(authority: &str, profile: &str, model: &str) -> InitializeModelSelection {
    InitializeModelSelection {
        authority_binding: authority.to_string(),
        selected_profile: profile.to_string(),
        model_id: model.to_string(),
        initialized_at: timestamp(0),
    }
}

fn selection_command(
    authority: &str,
    profile: &str,
    model: &str,
    key: &str,
    fingerprint: &str,
    occurred_at: chrono::DateTime<Utc>,
) -> SelectModel {
    SelectModel {
        authority_binding: authority.to_string(),
        selected_profile: profile.to_string(),
        model_id: model.to_string(),
        idempotency_key: key.to_string(),
        command_fingerprint: fingerprint.to_string(),
        occurred_at,
        host_event_publication: None,
    }
}

#[test]
fn first_read_persists_revision_one_and_first_default_wins() {
    let storage = SqliteStorage::in_memory().expect("storage");
    let first = storage
        .load_or_initialize_model_selection(initialization("authority-a", "coding", "model-a"))
        .expect("initialize selection");
    assert_eq!(first.revision, 1);
    assert_eq!(first.selected_profile, "coding");

    let loaded = storage
        .load_or_initialize_model_selection(initialization("authority-a", "general", "model-b"))
        .expect("load durable selection");
    assert_eq!(loaded, first);
}

#[test]
fn exact_replay_returns_historical_projection_without_mutating_receipt() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let database_path = tempdir.path().join("selection.sqlite3");
    let storage = SqliteStorage::open(&database_path).expect("storage");
    storage
        .load_or_initialize_model_selection(initialization("authority-a", "general", "model-a"))
        .expect("initialize");

    let first_command = selection_command(
        "authority-a",
        "coding",
        "model-b",
        "key-a",
        "sha256:fingerprint-a",
        timestamp(1),
    );
    let first = storage
        .select_model(first_command.clone())
        .expect("first selection");
    assert_eq!(first.selection.revision, 2);
    assert!(!first.receipt.replayed);

    let second = storage
        .select_model(selection_command(
            "authority-a",
            "research",
            "model-c",
            "key-b",
            "sha256:fingerprint-b",
            timestamp(2),
        ))
        .expect("second selection");
    assert_eq!(second.selection.revision, 3);

    let replay = storage.select_model(first_command).expect("exact replay");
    assert!(replay.receipt.replayed);
    assert_eq!(replay.selection, first.selection);

    let connection = Connection::open(database_path).expect("open receipt database");
    let payload: String = connection
        .query_row(
            "SELECT record FROM model_selection_mutation_receipts
             WHERE authority_binding = 'authority-a' AND idempotency_key = 'key-a'",
            [],
            |row| row.get(0),
        )
        .expect("durable receipt");
    let durable = from_versioned_json::<ModelSelectionMutationReceipt>(&payload)
        .expect("decode durable receipt");
    assert!(!durable.receipt.replayed);
    assert_eq!(durable, first);
}

#[test]
fn mismatched_fingerprint_returns_idempotency_conflict() {
    let storage = SqliteStorage::in_memory().expect("storage");
    storage
        .load_or_initialize_model_selection(initialization("authority-a", "general", "model-a"))
        .expect("initialize");
    storage
        .select_model(selection_command(
            "authority-a",
            "coding",
            "model-b",
            "key-a",
            "sha256:fingerprint-a",
            timestamp(1),
        ))
        .expect("first selection");

    let error = storage
        .select_model(selection_command(
            "authority-a",
            "coding",
            "model-b",
            "key-a",
            "sha256:different",
            timestamp(1),
        ))
        .expect_err("fingerprint conflict");
    assert!(matches!(error, SessionStoreError::IdempotencyConflict(_)));
}

#[tokio::test]
async fn state_receipt_and_host_event_outbox_roll_back_together() {
    let storage = SqliteStorage::in_memory().expect("storage");
    storage
        .load_or_initialize_model_selection(initialization("authority-a", "general", "model-a"))
        .expect("initialize");

    let original_event = PendingHostEventPublication::new(
        "occupied-transition",
        0,
        DurableHostEventScope::Global,
        DurableHostEventClass::Diagnostic,
        serde_json::json!({"state": "original"}),
        timestamp(1),
    )
    .expect("original event");
    storage
        .session_store()
        .enqueue_host_event_publications(vec![original_event.clone()])
        .await
        .expect("occupy publication key");

    let conflicting_event = PendingHostEventPublication::new(
        "occupied-transition",
        0,
        DurableHostEventScope::Global,
        DurableHostEventClass::Diagnostic,
        serde_json::json!({"state": "conflicting"}),
        timestamp(1),
    )
    .expect("conflicting event");
    let mut failed_command = selection_command(
        "authority-a",
        "coding",
        "model-b",
        "key-a",
        "sha256:fingerprint-a",
        timestamp(1),
    );
    failed_command.host_event_publication = Some(conflicting_event);
    storage
        .select_model(failed_command)
        .expect_err("outbox conflict must roll back transaction");

    let unchanged = storage
        .load_or_initialize_model_selection(initialization("authority-a", "ignored", "ignored"))
        .expect("load unchanged selection");
    assert_eq!(unchanged.revision, 1);
    assert_eq!(unchanged.selected_profile, "general");

    // The same key remains available, proving the receipt insert rolled back with state. This
    // successful retry moves directly from revision one to revision two.
    let committed = storage
        .select_model(selection_command(
            "authority-a",
            "coding",
            "model-b",
            "key-a",
            "sha256:fingerprint-a",
            timestamp(1),
        ))
        .expect("retry without conflicting event");
    assert_eq!(committed.selection.revision, 2);
    assert!(!committed.receipt.replayed);

    let pending = storage
        .session_store()
        .pending_host_event_publications(10)
        .await
        .expect("pending events");
    assert_eq!(pending, vec![original_event]);
}
