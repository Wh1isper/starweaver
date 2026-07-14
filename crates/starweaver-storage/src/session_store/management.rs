use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use starweaver_core::SessionId;
use starweaver_session::{
    AcquireRunAdmission, DurableControlReceipt, ManagedRunTarget, ManagedSessionTarget,
    RunAdmissionLease, RunAdmissionReceipt, RunStatus, SessionContinuationFence,
    SessionDeletionFence, SessionRecord, SessionStatus, SessionStoreError, SessionStoreResult,
    UpdateManagedSession,
};

use crate::sqlite::{deserialize_json_record, map_sqlite_session_error, serialize_json_record};

use super::{
    SqliteSessionStore,
    records::{
        allocate_or_reuse_run_sequence, apply_run_to_session, list_run_records, load_run_record,
        load_session_record, save_run_record, save_session_record,
    },
};

impl SqliteSessionStore {
    pub(super) fn create_session_idempotent_sync(
        &self,
        mut session: SessionRecord,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        if let Some(existing) = load_session_mutation_receipt(
            &transaction,
            &session.namespace_id,
            idempotency_key,
            command_fingerprint,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        if transaction
            .query_row(
                "SELECT 1 FROM session_records WHERE session_id = ?1",
                params![session.session_id.as_str()],
                |_row| Ok(()),
            )
            .optional()
            .map_err(map_sqlite_session_error)?
            .is_some()
        {
            return Err(SessionStoreError::AlreadyExists(
                session.session_id.as_str().to_string(),
            ));
        }
        session.revision = session.revision.max(1);
        session.updated_at = Utc::now();
        save_session_record(&transaction, &session)?;
        save_session_mutation_receipt(
            &transaction,
            &session,
            idempotency_key,
            command_fingerprint,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(session)
    }

    pub(super) fn update_managed_session_sync(
        &self,
        command: UpdateManagedSession,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, &command.session_id)?;
        if let Some(existing) = load_session_mutation_receipt(
            &transaction,
            &session.namespace_id,
            &command.idempotency_key,
            command_fingerprint,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        if session.revision != command.expected_revision {
            return Err(SessionStoreError::Conflict(format!(
                "expected revision {}, current {}",
                command.expected_revision, session.revision
            )));
        }
        if session.status == SessionStatus::Deleted || session.deletion_fence.blocks_continuation()
        {
            return Err(SessionStoreError::Conflict(
                "session is deleting or deleted".to_string(),
            ));
        }
        if let Some(title) = command.patch.title {
            session.title = title.map(|value| value.chars().take(256).collect());
        }
        if let Some(profile) = command.patch.profile {
            session.profile = profile;
        }
        if let Some(archived) = command.patch.archived {
            session.status = if archived {
                SessionStatus::Archived
            } else {
                SessionStatus::Active
            };
        }
        for (key, value) in command.patch.metadata {
            if value.is_null() {
                session.metadata.remove(&key);
            } else {
                session.metadata.insert(key, value);
            }
        }
        session.revision = session.revision.saturating_add(1);
        session.updated_at = Utc::now();
        save_session_record(&transaction, &session)?;
        save_session_mutation_receipt(
            &transaction,
            &session,
            &command.idempotency_key,
            command_fingerprint,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(session)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn acquire_session_deletion_fence_sync(
        &self,
        session_id: &SessionId,
        expected_revision: u64,
        fence_id: &str,
        requested_by: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, session_id)?;
        if let Some(existing) = load_session_mutation_receipt(
            &transaction,
            &session.namespace_id,
            idempotency_key,
            command_fingerprint,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        if session.revision != expected_revision {
            return Err(SessionStoreError::Conflict(format!(
                "expected revision {expected_revision}, current {}",
                session.revision
            )));
        }
        if !matches!(session.deletion_fence, SessionDeletionFence::Stable) {
            return Err(SessionStoreError::Conflict(
                "session already has a deletion fence".to_string(),
            ));
        }
        session.deletion_fence = SessionDeletionFence::Deleting {
            fence_id: fence_id.to_string(),
            expected_revision,
            requested_by: requested_by.to_string(),
            started_at: Utc::now(),
        };
        session.revision = session.revision.saturating_add(1);
        session.updated_at = Utc::now();
        save_session_record(&transaction, &session)?;
        save_session_mutation_receipt(
            &transaction,
            &session,
            idempotency_key,
            command_fingerprint,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(session)
    }

    pub(super) fn tombstone_session_sync(
        &self,
        session_id: &SessionId,
        fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, session_id)?;
        match &session.deletion_fence {
            SessionDeletionFence::Deleted {
                fence_id: current, ..
            } if current == fence_id => {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(session);
            }
            SessionDeletionFence::Deleting {
                fence_id: current, ..
            } if current == fence_id => {}
            _ => {
                return Err(SessionStoreError::Conflict(
                    "deletion fence mismatch".to_string(),
                ));
            }
        }
        let active = list_run_records(&transaction, session_id)?
            .into_iter()
            .any(|run| run.status.is_active());
        if active {
            return Err(SessionStoreError::RunConflict(
                "session still has an active run".to_string(),
            ));
        }
        session.status = SessionStatus::Deleted;
        session.active_run_id = None;
        session.deletion_fence = SessionDeletionFence::Deleted {
            fence_id: fence_id.to_string(),
            deleted_at: Utc::now(),
        };
        session.revision = session.revision.saturating_add(1);
        session.updated_at = Utc::now();
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(session)
    }

    pub(super) fn session_continuation_fence_sync(
        &self,
        namespace_id: &str,
        session_id: &SessionId,
    ) -> SessionStoreResult<SessionContinuationFence> {
        let connection = self.lock()?;
        let session = load_session_record(&connection, session_id)?;
        if session.namespace_id != namespace_id {
            return Err(SessionStoreError::NotFound(session_id.as_str().to_string()));
        }
        let fence_id = match &session.deletion_fence {
            SessionDeletionFence::Stable => None,
            SessionDeletionFence::Deleting { fence_id, .. }
            | SessionDeletionFence::Deleted { fence_id, .. } => Some(fence_id.clone()),
        };
        Ok(SessionContinuationFence {
            target: ManagedSessionTarget::new(namespace_id, session_id.clone()),
            revision: session.revision,
            continuation_allowed: !session.deletion_fence.blocks_continuation()
                && session.status != SessionStatus::Deleted,
            fence_id,
        })
    }

    pub(super) fn acquire_run_admission_sync(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        if let Some(mut existing) = load_admission_receipt(
            &transaction,
            &request.namespace_id,
            &request.idempotency_key,
            &request.command_fingerprint,
        )? {
            existing.idempotent_replay = true;
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        let mut session = load_session_record(&transaction, &request.run.session_id)?;
        if session.namespace_id != request.namespace_id {
            return Err(SessionStoreError::NotFound(
                request.run.session_id.as_str().to_string(),
            ));
        }
        if session.status != SessionStatus::Active || session.deletion_fence.blocks_continuation() {
            return Err(SessionStoreError::Conflict(
                "session cannot admit new work".to_string(),
            ));
        }
        if let Some(existing) =
            load_session_admission(&transaction, &request.namespace_id, &request.run.session_id)?
        {
            if !existing.expired_at(Utc::now()) {
                return Err(SessionStoreError::RunConflict(format!(
                    "session {} already has active run {}",
                    request.run.session_id.as_str(),
                    existing.target.run_id.as_str()
                )));
            }
            terminalize_orphan(&transaction, &existing, Utc::now())?;
            transaction
                .execute(
                    "DELETE FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                    params![request.namespace_id, request.run.session_id.as_str(), i64::try_from(existing.fencing_generation).unwrap_or(i64::MAX)],
                )
                .map_err(map_sqlite_session_error)?;
            session.active_run_id = None;
        } else if let Some(active_run_id) = session.active_run_id.clone() {
            let mut orphan = load_run_record(&transaction, &session.session_id, &active_run_id)?;
            if orphan.status.is_active() {
                orphan.status = RunStatus::Cancelled;
                orphan.output_preview =
                    Some("interrupted during admission reconciliation".to_string());
                orphan.updated_at = Utc::now();
                save_run_record(&transaction, &orphan)?;
            }
            session.active_run_id = None;
        }
        let generation =
            next_generation(&transaction, &request.namespace_id, &request.run.session_id)?;
        let mut run = request.run;
        run.status = RunStatus::Queued;
        run.updated_at = Utc::now();
        allocate_or_reuse_run_sequence(&transaction, &mut run)?;
        save_run_record(&transaction, &run)?;
        apply_run_to_session(&mut session, &run);
        session.revision = session.revision.saturating_add(1);
        save_session_record(&transaction, &session)?;
        let lease = RunAdmissionLease {
            target: ManagedRunTarget::new(
                request.namespace_id.clone(),
                run.session_id.clone(),
                run.run_id.clone(),
            ),
            admission_id: request.admission_id,
            host_instance_id: request.host_instance_id,
            fencing_generation: generation,
            lease_expires_at: request.lease_expires_at,
            heartbeat_at: Utc::now(),
            command_fingerprint: request.command_fingerprint.clone(),
            idempotency_key: request.idempotency_key.clone(),
        };
        transaction
            .execute(
                "INSERT INTO run_admissions
                 (namespace_id, session_id, run_id, generation, host_instance_id, lease_expires_at, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    lease.target.namespace_id,
                    lease.target.session_id.as_str(),
                    lease.target.run_id.as_str(),
                    i64::try_from(generation).unwrap_or(i64::MAX),
                    lease.host_instance_id,
                    lease.lease_expires_at.to_rfc3339(),
                    serialize_json_record(&lease)?,
                ],
            )
            .map_err(map_sqlite_session_error)?;
        let receipt = RunAdmissionReceipt {
            run,
            lease,
            idempotent_replay: false,
        };
        transaction
            .execute(
                "INSERT INTO run_admission_receipts
                 (namespace_id, idempotency_key, command_fingerprint, session_id, run_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    request.namespace_id,
                    request.idempotency_key,
                    request.command_fingerprint,
                    receipt.run.session_id.as_str(),
                    receipt.run.run_id.as_str(),
                    serialize_json_record(&receipt)?,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(receipt)
    }

    pub(super) fn heartbeat_run_admission_sync(
        &self,
        lease: &RunAdmissionLease,
        lease_expires_at: DateTime<Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut current = load_session_admission(
            &transaction,
            &lease.target.namespace_id,
            &lease.target.session_id,
        )?
        .ok_or_else(|| SessionStoreError::NotFound(lease.admission_id.clone()))?;
        ensure_same_owner(&current, lease)?;
        current.heartbeat_at = Utc::now();
        current.lease_expires_at = lease_expires_at;
        transaction
            .execute(
                "UPDATE run_admissions SET lease_expires_at = ?4, record = ?5
                 WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                params![
                    current.target.namespace_id,
                    current.target.session_id.as_str(),
                    i64::try_from(current.fencing_generation).unwrap_or(i64::MAX),
                    current.lease_expires_at.to_rfc3339(),
                    serialize_json_record(&current)?,
                ],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(current)
    }

    pub(super) fn release_run_admission_sync(
        &self,
        lease: &RunAdmissionLease,
    ) -> SessionStoreResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        if let Some(current) = load_session_admission(
            &transaction,
            &lease.target.namespace_id,
            &lease.target.session_id,
        )? {
            ensure_same_owner(&current, lease)?;
            transaction
                .execute(
                    "DELETE FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                    params![lease.target.namespace_id, lease.target.session_id.as_str(), i64::try_from(lease.fencing_generation).unwrap_or(i64::MAX)],
                )
                .map_err(map_sqlite_session_error)?;
        }
        transaction.commit().map_err(map_sqlite_session_error)
    }

    pub(super) fn load_run_admission_sync(
        &self,
        target: &ManagedRunTarget,
    ) -> SessionStoreResult<Option<RunAdmissionLease>> {
        let connection = self.lock()?;
        Ok(
            load_session_admission(&connection, &target.namespace_id, &target.session_id)?
                .filter(|lease| lease.target == *target),
        )
    }

    pub(super) fn reconcile_expired_run_admissions_sync(
        &self,
        namespace_id: &str,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<Vec<ManagedRunTarget>> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let payloads = {
            let mut statement = transaction
                .prepare(
                    "SELECT record FROM run_admissions WHERE namespace_id = ?1 AND lease_expires_at <= ?2 ORDER BY session_id",
                )
                .map_err(map_sqlite_session_error)?;
            statement
                .query_map(params![namespace_id, now.to_rfc3339()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        };
        let mut targets = Vec::with_capacity(payloads.len());
        for payload in payloads {
            let lease = deserialize_json_record::<RunAdmissionLease>(&payload)?;
            terminalize_orphan(&transaction, &lease, now)?;
            let mut session = load_session_record(&transaction, &lease.target.session_id)?;
            if session.active_run_id.as_ref() == Some(&lease.target.run_id) {
                session.active_run_id = None;
            }
            session.revision = session.revision.saturating_add(1);
            session.updated_at = now;
            save_session_record(&transaction, &session)?;
            transaction
                .execute(
                    "DELETE FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                    params![namespace_id, lease.target.session_id.as_str(), i64::try_from(lease.fencing_generation).unwrap_or(i64::MAX)],
                )
                .map_err(map_sqlite_session_error)?;
            targets.push(lease.target);
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(targets)
    }

    pub(super) fn load_control_receipt_sync(
        &self,
        target: &ManagedRunTarget,
        idempotency_key: &str,
    ) -> SessionStoreResult<Option<DurableControlReceipt>> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT record FROM run_control_receipts
                 WHERE namespace_id = ?1 AND session_id = ?2 AND run_id = ?3 AND idempotency_key = ?4",
                params![
                    target.namespace_id,
                    target.session_id.as_str(),
                    target.run_id.as_str(),
                    idempotency_key,
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?
            .map(|payload| deserialize_json_record(&payload))
            .transpose()
    }

    pub(super) fn reserve_control_receipt_sync(
        &self,
        receipt: DurableControlReceipt,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let existing = transaction
            .query_row(
                "SELECT command_fingerprint, record FROM run_control_receipts
                 WHERE namespace_id = ?1 AND session_id = ?2 AND run_id = ?3 AND idempotency_key = ?4",
                params![receipt.target.namespace_id, receipt.target.session_id.as_str(), receipt.target.run_id.as_str(), receipt.idempotency_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        if let Some((fingerprint, payload)) = existing {
            if fingerprint != receipt.command_fingerprint {
                return Err(SessionStoreError::IdempotencyConflict(
                    receipt.idempotency_key,
                ));
            }
            let existing = deserialize_json_record(&payload)?;
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        let lease = load_session_admission(
            &transaction,
            &receipt.target.namespace_id,
            &receipt.target.session_id,
        )?
        .ok_or_else(|| SessionStoreError::Conflict("run has no active owner lease".to_string()))?;
        if lease.target != receipt.target || lease.fencing_generation != receipt.fencing_generation
        {
            return Err(SessionStoreError::Conflict(
                "stale control generation".to_string(),
            ));
        }
        transaction
            .execute(
                "INSERT INTO run_control_receipts
                 (receipt_id, namespace_id, session_id, run_id, idempotency_key, command_fingerprint, generation, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    receipt.receipt_id,
                    receipt.target.namespace_id,
                    receipt.target.session_id.as_str(),
                    receipt.target.run_id.as_str(),
                    receipt.idempotency_key,
                    receipt.command_fingerprint,
                    i64::try_from(receipt.fencing_generation).unwrap_or(i64::MAX),
                    serialize_json_record(&receipt)?,
                    receipt.created_at.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(receipt)
    }

    pub(super) fn update_control_receipt_state_sync(
        &self,
        receipt_id: &str,
        state: &str,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let payload = transaction
            .query_row(
                "SELECT record FROM run_control_receipts WHERE receipt_id = ?1",
                params![receipt_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?
            .ok_or_else(|| SessionStoreError::NotFound(receipt_id.to_string()))?;
        let mut receipt = deserialize_json_record::<DurableControlReceipt>(&payload)?;
        receipt.state = state.to_string();
        transaction
            .execute(
                "UPDATE run_control_receipts SET record = ?2 WHERE receipt_id = ?1",
                params![receipt_id, serialize_json_record(&receipt)?],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(receipt)
    }
}

fn load_session_mutation_receipt(
    transaction: &Transaction<'_>,
    namespace_id: &str,
    idempotency_key: &str,
    command_fingerprint: &str,
) -> SessionStoreResult<Option<SessionRecord>> {
    let existing = transaction
        .query_row(
            "SELECT command_fingerprint, record FROM session_mutation_receipts
             WHERE namespace_id = ?1 AND idempotency_key = ?2",
            params![namespace_id, idempotency_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    existing
        .map(|(fingerprint, payload)| {
            if fingerprint != command_fingerprint {
                return Err(SessionStoreError::IdempotencyConflict(
                    idempotency_key.to_string(),
                ));
            }
            deserialize_json_record(&payload)
        })
        .transpose()
}

fn save_session_mutation_receipt(
    transaction: &Transaction<'_>,
    session: &SessionRecord,
    idempotency_key: &str,
    command_fingerprint: &str,
) -> SessionStoreResult<()> {
    transaction
        .execute(
            "INSERT INTO session_mutation_receipts
             (namespace_id, idempotency_key, command_fingerprint, session_id, record, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session.namespace_id,
                idempotency_key,
                command_fingerprint,
                session.session_id.as_str(),
                serialize_json_record(session)?,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn load_admission_receipt(
    transaction: &Transaction<'_>,
    namespace_id: &str,
    idempotency_key: &str,
    command_fingerprint: &str,
) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
    let existing = transaction
        .query_row(
            "SELECT command_fingerprint, record FROM run_admission_receipts
             WHERE namespace_id = ?1 AND idempotency_key = ?2",
            params![namespace_id, idempotency_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    existing
        .map(|(fingerprint, payload)| {
            if fingerprint != command_fingerprint {
                return Err(SessionStoreError::IdempotencyConflict(
                    idempotency_key.to_string(),
                ));
            }
            deserialize_json_record(&payload)
        })
        .transpose()
}

fn load_session_admission(
    connection: &rusqlite::Connection,
    namespace_id: &str,
    session_id: &SessionId,
) -> SessionStoreResult<Option<RunAdmissionLease>> {
    connection
        .query_row(
            "SELECT record FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2",
            params![namespace_id, session_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .map(|payload| deserialize_json_record(&payload))
        .transpose()
}

fn next_generation(
    transaction: &Transaction<'_>,
    namespace_id: &str,
    session_id: &SessionId,
) -> SessionStoreResult<u64> {
    transaction
        .execute(
            "INSERT INTO run_admission_generations (namespace_id, session_id, generation)
             VALUES (?1, ?2, 1)
             ON CONFLICT(namespace_id, session_id)
             DO UPDATE SET generation = generation + 1",
            params![namespace_id, session_id.as_str()],
        )
        .map_err(map_sqlite_session_error)?;
    let generation = transaction
        .query_row(
            "SELECT generation FROM run_admission_generations WHERE namespace_id = ?1 AND session_id = ?2",
            params![namespace_id, session_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_session_error)?;
    u64::try_from(generation)
        .map_err(|error| SessionStoreError::Failed(format!("invalid fencing generation: {error}")))
}

fn ensure_same_owner(
    current: &RunAdmissionLease,
    expected: &RunAdmissionLease,
) -> SessionStoreResult<()> {
    if current.target != expected.target
        || current.host_instance_id != expected.host_instance_id
        || current.fencing_generation != expected.fencing_generation
    {
        return Err(SessionStoreError::Conflict(
            "stale admission owner".to_string(),
        ));
    }
    Ok(())
}

fn terminalize_orphan(
    transaction: &Transaction<'_>,
    lease: &RunAdmissionLease,
    now: DateTime<Utc>,
) -> SessionStoreResult<()> {
    let mut run = load_run_record(transaction, &lease.target.session_id, &lease.target.run_id)?;
    if run.status.is_active() {
        run.status = RunStatus::Cancelled;
        run.output_preview = Some("interrupted after host lease expired".to_string());
        run.updated_at = now;
        save_run_record(transaction, &run)?;
    }
    Ok(())
}
