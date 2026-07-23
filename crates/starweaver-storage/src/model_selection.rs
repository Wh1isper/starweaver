//! Atomic SQLite persistence for authority-bound model selections.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use starweaver_session::{
    DurableModelSelection, InitializeModelSelection, MODEL_SELECTION_OPERATION,
    ModelSelectionMutationReceipt, MutationReceipt, SelectModel, SessionStoreError,
    SessionStoreResult,
};

use crate::{
    SqliteStorage,
    session_store::host_events::enqueue_host_event_publications_in_transaction,
    sqlite::{deserialize_json_record, map_sqlite_session_error, serialize_json_record},
};

impl SqliteStorage {
    /// Load an authority's durable model selection, atomically persisting the supplied default as
    /// revision one when no selection exists.
    ///
    /// Concurrent first reads are serialized with an SQLite `IMMEDIATE` transaction; the first
    /// committed default wins and later callers receive that durable selection.
    ///
    /// # Errors
    ///
    /// Returns a store error when input, persisted evidence, or SQLite state is invalid.
    pub fn load_or_initialize_model_selection(
        &self,
        initialization: InitializeModelSelection,
    ) -> SessionStoreResult<DurableModelSelection> {
        initialization.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;

        let selection = if let Some(selection) =
            load_selection(&transaction, &initialization.authority_binding)?
        {
            selection
        } else {
            let selection = DurableModelSelection {
                authority_binding: initialization.authority_binding,
                selected_profile: initialization.selected_profile,
                model_id: initialization.model_id,
                revision: 1,
                updated_at: initialization.initialized_at,
            };
            selection.validate()?;
            insert_selection(&transaction, &selection)?;
            selection
        };
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(selection)
    }

    /// Atomically update an authority's selection, persist its receipt, and optionally enqueue one
    /// durable host-event publication.
    ///
    /// The state record, original receipt (`replayed = false`), and event outbox row commit in one
    /// SQLite `IMMEDIATE` transaction. An exact retry returns the original state projection with a
    /// cloned receipt whose `replayed` field is true; it does not update any durable row or enqueue
    /// the event again. Reusing the key with another fingerprint returns
    /// [`SessionStoreError::IdempotencyConflict`].
    ///
    /// # Errors
    ///
    /// Returns `NotFound` when the authority has not been initialized, `IdempotencyConflict` for a
    /// mismatched key binding, or a store error for invalid evidence and SQLite failures.
    pub fn select_model(
        &self,
        command: SelectModel,
    ) -> SessionStoreResult<ModelSelectionMutationReceipt> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;

        if let Some((persisted_fingerprint, payload)) = load_receipt_row(
            &transaction,
            &command.authority_binding,
            &command.idempotency_key,
        )? {
            if persisted_fingerprint != command.command_fingerprint {
                return Err(SessionStoreError::IdempotencyConflict(format!(
                    "model selection key {} is already bound to another fingerprint",
                    command.idempotency_key
                )));
            }
            let persisted = deserialize_json_record::<ModelSelectionMutationReceipt>(&payload)?;
            persisted.validate()?;
            if persisted.receipt.fingerprint != persisted_fingerprint
                || persisted.receipt.idempotency_key != command.idempotency_key
            {
                return Err(SessionStoreError::Conflict(format!(
                    "durable model selection receipt {} does not match its key binding",
                    persisted.receipt.receipt_id
                )));
            }
            if persisted.receipt.replayed {
                return Err(SessionStoreError::Conflict(format!(
                    "durable model selection receipt {} was stored as a replay projection",
                    persisted.receipt.receipt_id
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(persisted.replayed_projection());
        }

        let current =
            load_selection(&transaction, &command.authority_binding)?.ok_or_else(|| {
                SessionStoreError::NotFound(format!(
                    "model selection for authority {}",
                    command.authority_binding
                ))
            })?;
        let revision = current.revision.checked_add(1).ok_or_else(|| {
            SessionStoreError::Conflict(format!(
                "model selection revision is exhausted for authority {}",
                command.authority_binding
            ))
        })?;
        i64::try_from(revision).map_err(|_| {
            SessionStoreError::Conflict(format!(
                "model selection revision exceeds SQLite range for authority {}",
                command.authority_binding
            ))
        })?;

        let selection = DurableModelSelection {
            authority_binding: command.authority_binding.clone(),
            selected_profile: command.selected_profile,
            model_id: command.model_id,
            revision,
            updated_at: command.occurred_at,
        };
        let receipt = MutationReceipt {
            receipt_id: mutation_receipt_id(&command.authority_binding, &command.idempotency_key),
            idempotency_key: command.idempotency_key.clone(),
            fingerprint: command.command_fingerprint.clone(),
            operation: MODEL_SELECTION_OPERATION.to_string(),
            state: "applied".to_string(),
            target_ref: command.authority_binding.clone(),
            reconciliation_required: false,
            replayed: false,
            created_at: command.occurred_at,
        };
        let result = ModelSelectionMutationReceipt { selection, receipt };
        result.validate()?;

        update_selection(&transaction, &result.selection, current.revision)?;
        insert_receipt(
            &transaction,
            &command.authority_binding,
            &command.command_fingerprint,
            &result,
        )?;
        if let Some(publication) = command.host_event_publication {
            enqueue_host_event_publications_in_transaction(&transaction, &[publication])?;
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }
}

fn load_selection(
    transaction: &Transaction<'_>,
    authority_binding: &str,
) -> SessionStoreResult<Option<DurableModelSelection>> {
    let payload = transaction
        .query_row(
            "SELECT record FROM model_selection_records WHERE authority_binding = ?1",
            params![authority_binding],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    let selection = payload
        .map(|payload| deserialize_json_record::<DurableModelSelection>(&payload))
        .transpose()?;
    if let Some(selection) = &selection {
        selection.validate()?;
        if selection.authority_binding != authority_binding {
            return Err(SessionStoreError::Conflict(format!(
                "model selection authority mismatch for {authority_binding}"
            )));
        }
    }
    Ok(selection)
}

fn insert_selection(
    transaction: &Transaction<'_>,
    selection: &DurableModelSelection,
) -> SessionStoreResult<()> {
    let revision = i64::try_from(selection.revision).map_err(|_| {
        SessionStoreError::Failed("model selection revision exceeds SQLite range".to_string())
    })?;
    let payload = serialize_json_record(selection)?;
    transaction
        .execute(
            "INSERT INTO model_selection_records
             (authority_binding, revision, record, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                selection.authority_binding,
                revision,
                payload,
                selection.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn update_selection(
    transaction: &Transaction<'_>,
    selection: &DurableModelSelection,
    expected_revision: u64,
) -> SessionStoreResult<()> {
    let revision = i64::try_from(selection.revision).map_err(|_| {
        SessionStoreError::Failed("model selection revision exceeds SQLite range".to_string())
    })?;
    let expected_revision = i64::try_from(expected_revision).map_err(|_| {
        SessionStoreError::Failed("model selection revision exceeds SQLite range".to_string())
    })?;
    let payload = serialize_json_record(selection)?;
    let updated = transaction
        .execute(
            "UPDATE model_selection_records
             SET revision = ?2, record = ?3, updated_at = ?4
             WHERE authority_binding = ?1 AND revision = ?5",
            params![
                selection.authority_binding,
                revision,
                payload,
                selection.updated_at.to_rfc3339(),
                expected_revision,
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "model selection revision changed for authority {}",
            selection.authority_binding
        )));
    }
    Ok(())
}

fn load_receipt_row(
    transaction: &Transaction<'_>,
    authority_binding: &str,
    idempotency_key: &str,
) -> SessionStoreResult<Option<(String, String)>> {
    transaction
        .query_row(
            "SELECT command_fingerprint, record
             FROM model_selection_mutation_receipts
             WHERE authority_binding = ?1 AND idempotency_key = ?2",
            params![authority_binding, idempotency_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_session_error)
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    authority_binding: &str,
    command_fingerprint: &str,
    result: &ModelSelectionMutationReceipt,
) -> SessionStoreResult<()> {
    let payload = serialize_json_record(result)?;
    transaction
        .execute(
            "INSERT INTO model_selection_mutation_receipts
             (authority_binding, idempotency_key, command_fingerprint, receipt_id, record,
              created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                authority_binding,
                result.receipt.idempotency_key,
                command_fingerprint,
                result.receipt.receipt_id,
                payload,
                result.receipt.created_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn mutation_receipt_id(authority_binding: &str, idempotency_key: &str) -> String {
    let mut digest = Sha256::new();
    for component in [
        MODEL_SELECTION_OPERATION,
        authority_binding,
        idempotency_key,
    ] {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    format!("mutation-receipt-sha256:{:x}", digest.finalize())
}
