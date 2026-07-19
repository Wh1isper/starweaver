use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use starweaver_core::SessionId;
use starweaver_session::{
    AcquireRunAdmission, ContinuationEffectState, DurableControlReceipt, HitlResumeClaim,
    HitlResumeClaimState, ManagedRunTarget, ManagedSessionTarget, RunAdmissionLease,
    RunAdmissionReceipt, RunRecord, RunStatus, RunTerminalError, RunTerminalProjection,
    SessionContinuationFence, SessionDeletionFence, SessionRecord, SessionStatus,
    SessionStoreError, SessionStoreResult, UpdateManagedSession,
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
    pub(crate) fn acquire_session_deletion_fence_sync(
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
        let now = Utc::now();
        if session.active_run_id.is_some()
            || load_session_admission(&transaction, &session.namespace_id, session_id)?.is_some()
        {
            return Err(SessionStoreError::RunConflict(
                "session still has an admitted active run".to_string(),
            ));
        }
        if has_active_background_ownership(&transaction, &session.namespace_id, session_id, now)? {
            return Err(SessionStoreError::RunConflict(
                "session still has active background-subagent ownership".to_string(),
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

    pub(crate) fn tombstone_session_sync(
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
        if has_active_background_ownership(
            &transaction,
            &session.namespace_id,
            session_id,
            Utc::now(),
        )? {
            return Err(SessionStoreError::RunConflict(
                "session still has active background-subagent ownership".to_string(),
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

    pub(crate) fn acquire_run_admission_sync(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let receipt = acquire_run_admission_in_transaction(&transaction, request)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(receipt)
    }

    pub(crate) fn load_run_admission_receipt_sync(
        &self,
        namespace_id: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
        let connection = self.lock()?;
        let mut receipt = load_admission_receipt(
            &connection,
            namespace_id,
            idempotency_key,
            command_fingerprint,
        )?;
        if let Some(receipt) = receipt.as_mut() {
            receipt.idempotent_replay = true;
        }
        Ok(receipt)
    }

    pub(crate) fn heartbeat_run_admission_sync(
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
        ensure_active_owner(&current, lease, Utc::now())?;
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

    pub(crate) fn release_run_admission_sync(
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
            ensure_active_owner(&current, lease, Utc::now())?;
            transaction
                .execute(
                    "DELETE FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                    params![lease.target.namespace_id, lease.target.session_id.as_str(), i64::try_from(lease.fencing_generation).unwrap_or(i64::MAX)],
                )
                .map_err(map_sqlite_session_error)?;
        }
        transaction.commit().map_err(map_sqlite_session_error)
    }

    pub(crate) fn update_run_status_fenced_sync(
        &self,
        lease: &RunAdmissionLease,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<starweaver_session::RunRecord> {
        if !status.is_active() {
            return Err(SessionStoreError::Conflict(
                "fenced status updates are non-terminal; use finalize_run_admission".to_string(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        ensure_run_admission_in_transaction(&transaction, lease, Utc::now())?;
        let mut run =
            load_run_record(&transaction, &lease.target.session_id, &lease.target.run_id)?;
        run.status = status;
        run.output_preview = output_preview;
        run.terminal_error = None;
        run.updated_at = Utc::now();
        save_run_record(&transaction, &run)?;
        let mut session = load_session_record(&transaction, &lease.target.session_id)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(run)
    }

    pub(crate) fn finalize_run_admission_sync(
        &self,
        lease: &RunAdmissionLease,
        terminal: RunTerminalProjection,
    ) -> SessionStoreResult<starweaver_session::RunRecord> {
        if terminal.status.is_active() {
            return Err(SessionStoreError::Conflict(
                "run admission can only finalize to a non-active status".to_string(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut run =
            load_run_record(&transaction, &lease.target.session_id, &lease.target.run_id)?;
        let current = load_session_admission(
            &transaction,
            &lease.target.namespace_id,
            &lease.target.session_id,
        )?;
        if current.is_none() {
            if terminal.matches(&run) {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(run);
            }
            return Err(SessionStoreError::Conflict(
                "stale admission owner".to_string(),
            ));
        }
        ensure_active_owner(
            current.as_ref().expect("checked active admission"),
            lease,
            Utc::now(),
        )?;
        if !run.status.is_terminal() {
            terminal
                .validate()
                .map_err(|error| SessionStoreError::Conflict(error.to_string()))?;
            terminal.apply_to(&mut run);
            run.updated_at = Utc::now();
            save_run_record(&transaction, &run)?;
            let mut session = load_session_record(&transaction, &lease.target.session_id)?;
            apply_run_to_session(&mut session, &run);
            save_session_record(&transaction, &session)?;
        }
        // Complete run evidence may be committed before its admission lease is released. In that
        // case this transaction owns only matching-lease cleanup and preserves the committed run,
        // even when a caller supplied a process-local fallback status.
        let deleted = transaction
            .execute(
                "DELETE FROM run_admissions
                 WHERE namespace_id = ?1 AND session_id = ?2 AND run_id = ?3
                   AND generation = ?4 AND host_instance_id = ?5",
                params![
                    lease.target.namespace_id,
                    lease.target.session_id.as_str(),
                    lease.target.run_id.as_str(),
                    i64::try_from(lease.fencing_generation).unwrap_or(i64::MAX),
                    lease.host_instance_id,
                ],
            )
            .map_err(map_sqlite_session_error)?;
        if deleted != 1 {
            return Err(SessionStoreError::Conflict(
                "stale admission owner".to_string(),
            ));
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(run)
    }

    pub(crate) fn load_run_admission_sync(
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

pub(super) fn acquire_run_admission_in_transaction(
    transaction: &Transaction<'_>,
    request: AcquireRunAdmission,
) -> SessionStoreResult<RunAdmissionReceipt> {
    if let Some(mut existing) = load_admission_receipt(
        transaction,
        &request.namespace_id,
        &request.idempotency_key,
        &request.command_fingerprint,
    )? {
        existing.idempotent_replay = true;
        return Ok(existing);
    }
    if request.replaces_waiting_run_id.is_some() != request.hitl_resume_claim_id.is_some() {
        return Err(SessionStoreError::Conflict(
            "waiting-run replacement requires exactly one preflight HITL claim".to_string(),
        ));
    }
    let mut session = load_session_record(transaction, &request.run.session_id)?;
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
        load_session_admission(transaction, &request.namespace_id, &request.run.session_id)?
    {
        if !existing.expired_at(Utc::now()) {
            return Err(SessionStoreError::RunConflict(format!(
                "session {} already has active run {}",
                request.run.session_id.as_str(),
                existing.target.run_id.as_str()
            )));
        }
        terminalize_orphan(transaction, &existing, Utc::now())?;
        transaction
            .execute(
                "DELETE FROM run_admissions WHERE namespace_id = ?1 AND session_id = ?2 AND generation = ?3",
                params![request.namespace_id, request.run.session_id.as_str(), i64::try_from(existing.fencing_generation).unwrap_or(i64::MAX)],
            )
            .map_err(map_sqlite_session_error)?;
        session.active_run_id = None;
    } else if let Some(active_run_id) = session.active_run_id.clone() {
        let source = load_run_record(transaction, &session.session_id, &active_run_id)?;
        let valid_waiting_replacement = request.replaces_waiting_run_id.as_ref()
            == Some(&active_run_id)
            && request.run.restore_from_run_id.as_ref() == Some(&active_run_id)
            && source.status == RunStatus::Waiting;
        if !valid_waiting_replacement {
            return Err(SessionStoreError::RunConflict(format!(
                "session {} already has active run {}",
                session.session_id.as_str(),
                active_run_id.as_str()
            )));
        }
    } else if let Some(source_run_id) = request.replaces_waiting_run_id.as_ref() {
        // A pre-effect replacement can clear the active pointer while preserving its waiting
        // source. Allow only that exact source to be retried under a new claim and replacement;
        // the source identity and Waiting status are still validated in this transaction.
        let source = load_run_record(transaction, &session.session_id, source_run_id)?;
        let valid_unparked_waiting_replacement = request.run.restore_from_run_id.as_ref()
            == Some(source_run_id)
            && source.status == RunStatus::Waiting;
        if !valid_unparked_waiting_replacement {
            return Err(SessionStoreError::Conflict(
                "waiting-run replacement has no retryable waiting source".to_string(),
            ));
        }
    }
    let mut hitl_claim = match (
        request.replaces_waiting_run_id.as_ref(),
        request.hitl_resume_claim_id.as_deref(),
    ) {
        (Some(source_run_id), Some(claim_id)) => {
            let payload = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
                    params![request.run.session_id.as_str(), source_run_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?
                .ok_or_else(|| {
                    SessionStoreError::NotFound(format!(
                        "resume claim for {}",
                        source_run_id.as_str()
                    ))
                })?;
            let claim = deserialize_json_record::<HitlResumeClaim>(&payload)?;
            if claim.claim_id != claim_id
                || claim.session_id != request.run.session_id
                || claim.run_id != *source_run_id
                || claim.state != HitlResumeClaimState::Preflight
            {
                return Err(SessionStoreError::Conflict(format!(
                    "invalid preflight resume claim for run {}",
                    source_run_id.as_str()
                )));
            }
            Some(claim)
        }
        (None, None) => None,
        _ => unreachable!("replacement and claim presence checked above"),
    };
    let generation = next_generation(transaction, &request.namespace_id, &request.run.session_id)?;
    let mut run = request.run;
    run.normalize_for_admission();
    run.validate_new_write().map_err(|error| {
        SessionStoreError::Failed(format!(
            "invalid admitted run state for {}: {error}",
            run.run_id.as_str()
        ))
    })?;
    run.updated_at = Utc::now();
    allocate_or_reuse_run_sequence(transaction, &mut run)?;
    save_run_record(transaction, &run)?;
    apply_run_to_session(&mut session, &run);
    session.revision = session.revision.saturating_add(1);
    save_session_record(transaction, &session)?;
    if let Some(claim) = hitl_claim.as_mut() {
        claim.state = HitlResumeClaimState::Admitted;
        let updated = transaction
            .execute(
                "UPDATE hitl_resume_claims SET record = ?3
                 WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?4",
                params![
                    claim.session_id.as_str(),
                    claim.run_id.as_str(),
                    serialize_json_record(claim)?,
                    claim.claim_id,
                ],
            )
            .map_err(map_sqlite_session_error)?;
        if updated != 1 {
            return Err(SessionStoreError::Conflict(format!(
                "resume claim changed during admission for run {}",
                claim.run_id.as_str()
            )));
        }
    }
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
    Ok(receipt)
}

fn has_active_background_ownership(
    transaction: &Transaction<'_>,
    namespace_id: &str,
    session_id: &SessionId,
    now: DateTime<Utc>,
) -> SessionStoreResult<bool> {
    transaction
        .query_row(
            "SELECT 1 FROM background_subagent_records
             WHERE namespace_id = ?1 AND parent_session_id = ?2
               AND execution_status IN ('accepted', 'starting', 'running', 'waiting')
               AND owner_lease_expires_at > ?3
             LIMIT 1",
            params![namespace_id, session_id.as_str(), now.to_rfc3339()],
            |_row| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(map_sqlite_session_error)
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
    connection: &rusqlite::Connection,
    namespace_id: &str,
    idempotency_key: &str,
    command_fingerprint: &str,
) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
    let existing = connection
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

pub(super) fn load_session_admission(
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

pub fn ensure_run_admission_in_transaction(
    transaction: &Transaction<'_>,
    expected: &RunAdmissionLease,
    now: DateTime<Utc>,
) -> SessionStoreResult<()> {
    let current = load_session_admission(
        transaction,
        &expected.target.namespace_id,
        &expected.target.session_id,
    )?
    .ok_or_else(|| SessionStoreError::StaleFence("run has no active owner lease".to_string()))?;
    ensure_active_owner(&current, expected, now)
}

fn ensure_same_owner(
    current: &RunAdmissionLease,
    expected: &RunAdmissionLease,
) -> SessionStoreResult<()> {
    if current.target != expected.target
        || current.admission_id != expected.admission_id
        || current.host_instance_id != expected.host_instance_id
        || current.fencing_generation != expected.fencing_generation
    {
        return Err(SessionStoreError::StaleFence(
            "stale admission owner".to_string(),
        ));
    }
    Ok(())
}

fn ensure_active_owner(
    current: &RunAdmissionLease,
    expected: &RunAdmissionLease,
    now: DateTime<Utc>,
) -> SessionStoreResult<()> {
    ensure_same_owner(current, expected)?;
    if current.expired_at(now) {
        return Err(SessionStoreError::StaleFence(
            "run admission lease expired".to_string(),
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
        let effect_started = reconcile_hitl_source_for_orphan(transaction, &run, now)?;
        run.status = RunStatus::Cancelled;
        run.output_preview = Some("interrupted after host lease expired".to_string());
        run.terminal_error = Some(RunTerminalError::new(
            "admission_lease_expired",
            "interrupted after host lease expired",
        ));
        run.updated_at = now;
        if effect_started {
            ContinuationEffectState::indeterminate()
                .insert_into(&mut run.metadata)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        }
        save_run_record(transaction, &run)?;
    }
    Ok(())
}

fn reconcile_hitl_source_for_orphan(
    transaction: &Transaction<'_>,
    replacement: &RunRecord,
    now: DateTime<Utc>,
) -> SessionStoreResult<bool> {
    let Some(source_run_id) = replacement.restore_from_run_id.as_ref() else {
        return Ok(false);
    };
    let mut source = load_run_record(transaction, &replacement.session_id, source_run_id)?;
    if source.status != RunStatus::Waiting {
        return Ok(false);
    }
    let persisted = transaction
        .query_row(
            "SELECT record FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
            params![replacement.session_id.as_str(), source_run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| {
            SessionStoreError::Conflict(format!(
                "waiting replacement source {} has no resume claim",
                source_run_id.as_str()
            ))
        })?;
    let claim = deserialize_json_record::<HitlResumeClaim>(&persisted)?;
    if claim.session_id != replacement.session_id || claim.run_id != *source_run_id {
        return Err(SessionStoreError::Conflict(format!(
            "waiting replacement source {} has a mismatched resume claim",
            source_run_id.as_str()
        )));
    }
    match claim.state {
        // No approved tool can have executed before this boundary. Discarding an orphaned
        // admitted claim restores the source waiting run for a fresh (new-key) continuation.
        HitlResumeClaimState::Admitted => {}
        // An approved tool may have executed after `Started`, so never make the source eligible
        // for automatic retry. Persist a typed indeterminate projection for host recovery UI.
        HitlResumeClaimState::Started => {
            source.status = RunStatus::Cancelled;
            source.output_preview = Some("interrupted after host lease expired".to_string());
            source.terminal_error = Some(RunTerminalError::new(
                "admission_lease_expired",
                "interrupted after host lease expired",
            ));
            source.updated_at = now;
            ContinuationEffectState::indeterminate()
                .insert_into(&mut source.metadata)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            save_run_record(transaction, &source)?;
        }
        HitlResumeClaimState::Preflight => {
            return Err(SessionStoreError::Conflict(format!(
                "waiting replacement source {} has an invalid preflight claim",
                source_run_id.as_str()
            )));
        }
    }
    let deleted = transaction
        .execute(
            "DELETE FROM hitl_resume_claims
             WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?3 AND record = ?4",
            params![
                claim.session_id.as_str(),
                claim.run_id.as_str(),
                claim.claim_id,
                persisted,
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if deleted != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "resume claim changed while reconciling run {}",
            source_run_id.as_str()
        )));
    }
    Ok(claim.state == HitlResumeClaimState::Started)
}
