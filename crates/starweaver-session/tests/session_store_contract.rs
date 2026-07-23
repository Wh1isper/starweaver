#![allow(clippy::unwrap_used, missing_docs)]

use std::sync::Arc;

use starweaver_session::{InMemorySessionStore, SessionStore};

#[path = "support/session_store_contract.rs"]
mod contract;

#[tokio::test]
async fn in_memory_store_satisfies_shared_session_store_contract() {
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    Box::pin(contract::assert_stable_session_page_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(contract::assert_session_store_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(
        contract::assert_approval_reviewed_arguments_immutable_contract(store.clone(), "memory"),
    )
    .await;
    Box::pin(contract::assert_atomic_hitl_replacement_admission_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(contract::assert_started_hitl_orphan_reconciliation_contract(store.clone(), "memory"))
        .await;
    Box::pin(
        contract::assert_implicit_started_hitl_orphan_reconciliation_contract(
            store.clone(),
            "memory",
        ),
    )
    .await;
    Box::pin(contract::assert_fenced_replay_batch_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(
        contract::assert_terminal_evidence_admission_cleanup_contract(store.clone(), "memory"),
    )
    .await;
    Box::pin(contract::assert_durable_host_event_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(contract::assert_atomic_session_host_event_contract(
        store.clone(),
        "memory",
    ))
    .await;
    Box::pin(contract::assert_background_subagent_contract(
        store, "memory",
    ))
    .await;
}
