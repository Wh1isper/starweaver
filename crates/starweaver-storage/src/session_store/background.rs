use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use starweaver_core::{SessionId, SubagentAttemptId};
use starweaver_session::{
    AcquireBackgroundSubagentContinuation, BackgroundSubagentArtifact,
    BackgroundSubagentArtifactLimits, BackgroundSubagentContinuationCause,
    BackgroundSubagentContinuationReceipt, BackgroundSubagentRecord,
    BackgroundSubagentTerminalCommit, DurableBackgroundSubagentDeliveryClaim,
    DurableBackgroundSubagentDeliveryRelease, DurableBackgroundSubagentDeliveryStatus,
    DurableBackgroundSubagentExecutionStatus, DurableBackgroundSubagentResultRef,
    RunAdmissionReceipt, RunRecord, RunStatus, SessionStatus, SessionStoreError,
    SessionStoreResult,
};

use crate::sqlite::{deserialize_json_record, map_sqlite_session_error, serialize_json_record};

use super::{
    SqliteSessionStore,
    management::{acquire_run_admission_in_transaction, load_session_admission},
    records::{load_run_record, load_session_record},
};

impl SqliteSessionStore {
    pub(super) fn record_background_subagent_acceptance_sync(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        if !record.is_valid_acceptance() {
            return Err(SessionStoreError::Conflict(
                "invalid background-subagent acceptance record".to_string(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        if let Some(existing) = load_background_record_optional(&transaction, &record.attempt_id)? {
            if !same_background_identity(&existing, &record) {
                return Err(SessionStoreError::Conflict(format!(
                    "background attempt {} already exists with different identity",
                    record.attempt_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(existing);
        }
        if record.owner_lease.expired_at(Utc::now()) {
            return Err(SessionStoreError::Conflict(
                "background-subagent acceptance owner lease is already expired".to_string(),
            ));
        }
        let active_agent_attempt = transaction
            .query_row(
                "SELECT attempt_id FROM background_subagent_records
                 WHERE namespace_id = ?1 AND parent_session_id = ?2 AND agent_id = ?3
                   AND execution_status IN ('accepted', 'starting', 'running', 'waiting')
                 LIMIT 1",
                params![
                    record.namespace_id,
                    record.parent_session_id.as_str(),
                    record.agent_id
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        if active_agent_attempt.is_some() {
            return Err(SessionStoreError::Conflict(format!(
                "background agent {} already has an active durable attempt",
                record.agent_id
            )));
        }
        let session = load_session_record(&transaction, &record.parent_session_id)?;
        if session.namespace_id != record.namespace_id
            || session.status != SessionStatus::Active
            || session.deletion_fence.blocks_continuation()
        {
            return Err(SessionStoreError::Conflict(
                "session cannot admit background delegation".to_string(),
            ));
        }
        load_run_record(
            &transaction,
            &record.parent_session_id,
            &record.parent_run_id,
        )?;
        save_background_record(&transaction, &record, true)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn update_background_subagent_execution_sync(
        &self,
        mut record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let current = load_background_record(&transaction, &record.attempt_id)?;
        ensure_background_parent_writable(&transaction, &current)?;
        if current.owner_lease.expired_at(Utc::now())
            || !same_background_identity(&current, &record)
            || !same_background_owner(&current, &record)
            || current.execution_status.is_terminal()
            || record.execution_status.is_terminal()
            || !valid_background_transition(current.execution_status, record.execution_status)
            || !same_background_non_execution_state(&current, &record)
        {
            return Err(SessionStoreError::Conflict(format!(
                "invalid background execution transition for {}",
                record.attempt_id.as_str()
            )));
        }
        record.owner_lease = current.owner_lease;
        save_background_record(&transaction, &record, false)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn heartbeat_background_subagent_sync(
        &self,
        attempt_id: &SubagentAttemptId,
        host_instance_id: &str,
        fencing_generation: u64,
        lease_expires_at: DateTime<Utc>,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let now = Utc::now();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut record = load_background_record(&transaction, attempt_id)?;
        ensure_background_parent_writable(&transaction, &record)?;
        if record.execution_status.is_terminal()
            || record.owner_lease.expired_at(now)
            || record.owner_lease.host_instance_id != host_instance_id
            || record.owner_lease.fencing_generation != fencing_generation
            || lease_expires_at <= now
        {
            return Err(SessionStoreError::Conflict(
                "stale or invalid background-subagent owner heartbeat".to_string(),
            ));
        }
        record.owner_lease.heartbeat_at = now;
        record.owner_lease.lease_expires_at = lease_expires_at;
        save_background_record(&transaction, &record, false)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn commit_background_subagent_terminal_sync(
        &self,
        commit: BackgroundSubagentTerminalCommit,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let record = commit_background_terminal_transaction(
            &transaction,
            commit.record,
            commit.artifact,
            commit.artifact_limits,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn load_background_subagent_artifact_sync(
        &self,
        artifact_ref: &str,
    ) -> SessionStoreResult<BackgroundSubagentArtifact> {
        let connection = self.lock()?;
        let payload = connection
            .query_row(
                "SELECT artifact FROM background_subagent_artifacts WHERE artifact_ref = ?1",
                params![artifact_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?
            .ok_or_else(|| SessionStoreError::NotFound(artifact_ref.to_string()))?;
        let artifact = deserialize_json_record::<BackgroundSubagentArtifact>(&payload)?;
        if artifact.expires_at <= Utc::now() {
            return Err(SessionStoreError::NotFound(artifact_ref.to_string()));
        }
        if !artifact.is_valid() {
            return Err(SessionStoreError::Conflict(
                "background-subagent artifact failed integrity validation".to_string(),
            ));
        }
        Ok(artifact)
    }

    pub(super) fn expire_background_subagent_retention_sync(
        &self,
        namespace_id: &str,
        now: DateTime<Utc>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let attempt_ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT attempt_id FROM background_subagent_records
                     WHERE namespace_id = ?1 AND retention_status != 'expired'
                       AND retention_expires_at IS NOT NULL AND retention_expires_at <= ?2
                     ORDER BY retention_expires_at, attempt_id LIMIT ?3",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(
                    params![
                        namespace_id,
                        now.to_rfc3339(),
                        i64::try_from(limit).unwrap_or(i64::MAX)
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        };
        let mut expired = Vec::with_capacity(attempt_ids.len());
        for attempt_id in attempt_ids {
            let attempt_id = SubagentAttemptId::from_string(attempt_id);
            let mut record = load_background_record(&transaction, &attempt_id)?;
            if !record.execution_status.is_terminal()
                || record.retention_status
                    == starweaver_session::DurableBackgroundSubagentRetentionStatus::Expired
                || record
                    .retention_expires_at
                    .is_none_or(|deadline| deadline > now)
            {
                continue;
            }
            if let Some(fingerprint) =
                persisted_background_terminal_fingerprint(&transaction, &record)?
            {
                save_background_terminal_fingerprint(
                    &transaction,
                    &record.attempt_id,
                    &fingerprint,
                )?;
            }
            if let Some(result_ref) = record.result_ref.as_mut() {
                if let Some(artifact_ref) = result_ref.artifact_ref.take() {
                    transaction
                        .execute(
                            "DELETE FROM background_subagent_artifacts WHERE artifact_ref = ?1",
                            params![artifact_ref],
                        )
                        .map_err(map_sqlite_session_error)?;
                }
                result_ref.content = None;
                result_ref.error = None;
            }
            record.retention_status =
                starweaver_session::DurableBackgroundSubagentRetentionStatus::Expired;
            record.retention_expires_at = None;
            save_background_record(&transaction, &record, false)?;
            expired.push(record);
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(expired)
    }

    pub(super) fn record_background_subagent_terminal_sync(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let record = commit_background_terminal_transaction(&transaction, record, None, None)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn load_background_subagent_sync(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let connection = self.lock()?;
        load_background_record(&connection, attempt_id)
    }

    pub(super) fn list_background_subagents_sync(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let connection = self.lock()?;
        let sql = if session_id.is_some() {
            "SELECT record FROM background_subagent_records
             WHERE namespace_id = ?1 AND parent_session_id = ?2
             ORDER BY updated_at DESC, attempt_id ASC LIMIT ?3"
        } else {
            "SELECT record FROM background_subagent_records
             WHERE namespace_id = ?1
             ORDER BY updated_at DESC, attempt_id ASC LIMIT ?3"
        };
        let mut statement = connection.prepare(sql).map_err(map_sqlite_session_error)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let payloads = if let Some(session_id) = session_id {
            statement
                .query_map(params![namespace_id, session_id.as_str(), limit], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        } else {
            statement
                .query_map(
                    params![namespace_id, Option::<String>::None, limit],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        };
        payloads
            .into_iter()
            .map(|payload| deserialize_json_record(&payload))
            .collect()
    }

    pub(super) fn list_pending_background_subagents_sync(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let connection = self.lock()?;
        let sql = if session_id.is_some() {
            "SELECT record FROM background_subagent_records
             WHERE namespace_id = ?1 AND parent_session_id = ?2
               AND execution_status IN ('completed', 'failed', 'cancelled')
               AND delivery_status != 'delivered'
             ORDER BY updated_at ASC, attempt_id ASC LIMIT ?3"
        } else {
            "SELECT record FROM background_subagent_records
             WHERE namespace_id = ?1
               AND execution_status IN ('completed', 'failed', 'cancelled')
               AND delivery_status != 'delivered'
             ORDER BY updated_at ASC, attempt_id ASC LIMIT ?3"
        };
        let mut statement = connection.prepare(sql).map_err(map_sqlite_session_error)?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let payloads = if let Some(session_id) = session_id {
            statement
                .query_map(params![namespace_id, session_id.as_str(), limit], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        } else {
            statement
                .query_map(
                    params![namespace_id, Option::<String>::None, limit],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        };
        payloads
            .into_iter()
            .map(|payload| deserialize_json_record(&payload))
            .collect()
    }

    pub(super) fn claim_background_subagent_delivery_sync(
        &self,
        attempt_id: &SubagentAttemptId,
        claim: DurableBackgroundSubagentDeliveryClaim,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let now = Utc::now();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut record = load_background_record(&transaction, attempt_id)?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
            && record.delivery_claim.as_ref() == Some(&claim)
        {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(record);
        }
        if let Some(current_claim) = record.delivery_claim.clone()
            && record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
            && current_claim.deadline <= now
        {
            match background_claim_consumer_state(&transaction, &record, &current_claim, now)? {
                BackgroundClaimConsumerState::Live => {
                    return Err(SessionStoreError::Conflict(
                        "live admitted consumer still owns the background delivery claim"
                            .to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Completed(run_id) => {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
                    record.delivery_claim = None;
                    record.delivered_claim_id = Some(current_claim.claim_id);
                    record.continuation_run_id = Some(run_id);
                    record.automatic_continuation_suppressed_by_run_id = None;
                    record.updated_at = now;
                    save_background_record(&transaction, &record, false)?;
                    transaction.commit().map_err(map_sqlite_session_error)?;
                    return Err(SessionStoreError::Conflict(
                        "completed consumer already delivered the background result".to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Terminated(run_id) => {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                    record.delivery_claim = None;
                    record.automatic_continuation_suppressed_by_run_id = Some(run_id);
                    record.updated_at = now;
                    save_background_record(&transaction, &record, false)?;
                    transaction.commit().map_err(map_sqlite_session_error)?;
                    return Err(SessionStoreError::Conflict(
                        "terminated consumer released the background delivery claim".to_string(),
                    ));
                }
                BackgroundClaimConsumerState::Reclaimable => {}
            }
        }
        if !record.delivery_claimable_at(now) {
            return Err(SessionStoreError::Conflict(
                "background result delivery is not claimable".to_string(),
            ));
        }
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Claimed;
        record.delivery_claim = Some(claim);
        record.updated_at = now;
        save_background_record(&transaction, &record, false)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn acknowledge_background_subagent_delivery_sync(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut record = load_background_record(&transaction, attempt_id)?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Delivered {
            if record.delivered_claim_id.as_deref() != Some(claim_id) {
                return Err(SessionStoreError::Conflict(
                    "background result was delivered by another claim".to_string(),
                ));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(record);
        }
        if record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Claimed
            || record
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
        {
            return Err(SessionStoreError::Conflict(
                "background delivery claim mismatch".to_string(),
            ));
        }
        record.continuation_run_id = record
            .delivery_claim
            .as_ref()
            .and_then(|claim| claim.continuation_run_id.clone());
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
        record.delivery_claim = None;
        record.delivered_claim_id = Some(claim_id.to_string());
        record.automatic_continuation_suppressed_by_run_id = None;
        record.updated_at = Utc::now();
        save_background_record(&transaction, &record, false)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn release_background_subagent_delivery_sync(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
        release: DurableBackgroundSubagentDeliveryRelease,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut record = load_background_record(&transaction, attempt_id)?;
        if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered {
            return match release {
                DurableBackgroundSubagentDeliveryRelease::Retryable => {
                    transaction.commit().map_err(map_sqlite_session_error)?;
                    Ok(record)
                }
                DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated { .. } => {
                    Err(SessionStoreError::Conflict(
                        "terminated consumer has no matching background delivery claim".to_string(),
                    ))
                }
            };
        }
        if record.delivery_status != DurableBackgroundSubagentDeliveryStatus::Claimed
            || record
                .delivery_claim
                .as_ref()
                .is_none_or(|claim| claim.claim_id != claim_id)
        {
            return Err(SessionStoreError::Conflict(
                "background delivery claim mismatch".to_string(),
            ));
        }
        if let DurableBackgroundSubagentDeliveryRelease::ConsumerTerminated { run_id } = release {
            if record
                .delivery_claim
                .as_ref()
                .and_then(|claim| claim.continuation_run_id.as_ref())
                != Some(&run_id)
                || background_claim_consumer_state(
                    &transaction,
                    &record,
                    record.delivery_claim.as_ref().ok_or_else(|| {
                        SessionStoreError::Conflict(
                            "claimed background result is missing its claim".to_string(),
                        )
                    })?,
                    Utc::now(),
                )? != BackgroundClaimConsumerState::Terminated(run_id.clone())
            {
                return Err(SessionStoreError::Conflict(
                    "terminated consumer does not own the claim or is not terminal".to_string(),
                ));
            }
            record.automatic_continuation_suppressed_by_run_id = Some(run_id);
        }
        record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
        record.delivery_claim = None;
        record.updated_at = Utc::now();
        save_background_record(&transaction, &record, false)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(record)
    }

    pub(super) fn acquire_background_subagent_continuation_sync(
        &self,
        request: AcquireBackgroundSubagentContinuation,
    ) -> SessionStoreResult<BackgroundSubagentContinuationReceipt> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut background = load_background_record(&transaction, &request.attempt_id)?;
        let continuation_run_id = request.admission.run.run_id.clone();
        if request.attempt_id != request.cause.attempt_id
            || background
                .automatic_continuation_suppressed_by_run_id
                .is_some()
            || !background
                .validates_continuation_cause_envelope(&request.cause, &request.admission.run)
        {
            return Err(SessionStoreError::Conflict(
                "background continuation cause does not match its durable result".to_string(),
            ));
        }
        if background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Delivered
            || background.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
        {
            let same_claim = background.continuation_run_id.as_ref() == Some(&continuation_run_id)
                && (background.delivered_claim_id.as_deref() == Some(&request.claim_id)
                    || background
                        .delivery_claim
                        .as_ref()
                        .is_some_and(|claim| claim.claim_id == request.claim_id));
            if !same_claim {
                return Err(SessionStoreError::Conflict(
                    "background result was already claimed or delivered".to_string(),
                ));
            }
            let mut admission = load_run_admission_receipt(
                &transaction,
                &request.admission.namespace_id,
                &request.admission.idempotency_key,
                &request.admission.command_fingerprint,
            )?
            .ok_or_else(|| SessionStoreError::NotFound(request.claim_id.clone()))?;
            admission.idempotent_replay = true;
            if !continuation_receipt_matches_request(&background, &request, &admission) {
                return Err(SessionStoreError::Conflict(
                    "stored continuation receipt does not match the causal request".to_string(),
                ));
            }
            let cause = BackgroundSubagentContinuationCause::new(&background, &admission.run.input)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(BackgroundSubagentContinuationReceipt {
                cause,
                background,
                admission,
            });
        }
        let now = Utc::now();
        let artifact_content = continuation_artifact_content(&transaction, &background, now)?;
        if background.parent_session_id != request.admission.run.session_id
            || background.namespace_id != request.admission.namespace_id
            || !background.validates_continuation_cause(
                &request.cause,
                &request.admission.run,
                artifact_content.as_deref(),
            )
            || request.claim_id.is_empty()
            || request.claim_deadline <= now
            || !background.delivery_claimable_at(now)
        {
            return Err(SessionStoreError::Conflict(
                "background result cannot admit this continuation".to_string(),
            ));
        }
        let session = load_session_record(&transaction, &background.parent_session_id)?;
        if request.admission.run.restore_from_run_id != session.head_run_id {
            return Err(SessionStoreError::Conflict(
                "background continuation source no longer matches the session head".to_string(),
            ));
        }
        let admission_request = request.clone();
        let admission =
            acquire_run_admission_in_transaction(&transaction, request.admission.clone())?;
        if !continuation_receipt_matches_request(&background, &admission_request, &admission) {
            return Err(SessionStoreError::Conflict(
                "admitted continuation receipt lost causal input binding".to_string(),
            ));
        }
        background.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
        background.delivery_claim = None;
        background.delivered_claim_id = Some(request.claim_id);
        background.continuation_run_id = Some(admission.run.run_id.clone());
        background.updated_at = Utc::now();
        save_background_record(&transaction, &background, false)?;
        let cause = BackgroundSubagentContinuationCause::new(&background, &admission.run.input)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(BackgroundSubagentContinuationReceipt {
            cause,
            background,
            admission,
        })
    }

    pub(super) fn reconcile_background_subagents_sync(
        &self,
        namespace_id: &str,
        now: DateTime<Utc>,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let payloads = {
            let mut statement = transaction
                .prepare(
                    "SELECT record FROM background_subagent_records
                     WHERE namespace_id = ?1 ORDER BY updated_at, attempt_id",
                )
                .map_err(map_sqlite_session_error)?;
            statement
                .query_map(params![namespace_id], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        };
        let mut changed = Vec::new();
        for payload in payloads {
            let mut record = deserialize_json_record::<BackgroundSubagentRecord>(&payload)?;
            if !record.execution_status.is_terminal() && record.owner_lease.expired_at(now) {
                let error = "in-process background execution was interrupted by host restart";
                record.execution_status = DurableBackgroundSubagentExecutionStatus::Failed;
                record.failure_category = Some("host_process_lost".to_string());
                record.result_ref = Some(DurableBackgroundSubagentResultRef {
                    error: Some(error.to_string()),
                    size_bytes: u64::try_from(error.len()).unwrap_or(u64::MAX),
                    ..DurableBackgroundSubagentResultRef::default()
                });
                record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                record.delivery_claim = None;
                record.retention_expires_at = Some(
                    now + chrono::Duration::seconds(
                        starweaver_session::DEFAULT_BACKGROUND_RESULT_RETENTION_SECS,
                    ),
                );
                record.terminal_at = Some(now);
                record.updated_at = now;
                let terminal_fingerprint = background_terminal_fingerprint(&record, None)?;
                save_background_record(&transaction, &record, false)?;
                save_background_terminal_fingerprint(
                    &transaction,
                    &record.attempt_id,
                    &terminal_fingerprint,
                )?;
                changed.push(record);
            } else if record.delivery_status == DurableBackgroundSubagentDeliveryStatus::Claimed
                && record
                    .delivery_claim
                    .as_ref()
                    .is_some_and(|claim| claim.deadline <= now)
            {
                let claim = record.delivery_claim.clone().ok_or_else(|| {
                    SessionStoreError::Conflict(
                        "claimed background result is missing its claim".to_string(),
                    )
                })?;
                let consumer = if let Some(run_id) = claim.continuation_run_id.as_ref() {
                    transaction
                        .query_row(
                            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
                            params![record.parent_session_id.as_str(), run_id.as_str()],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(map_sqlite_session_error)?
                        .map(|payload| deserialize_json_record::<RunRecord>(&payload))
                        .transpose()?
                } else {
                    None
                };
                let live_admission = load_session_admission(
                    &transaction,
                    &record.namespace_id,
                    &record.parent_session_id,
                )?;
                let consumer_has_live_admission = consumer.as_ref().is_some_and(|run| {
                    run.status.is_active()
                        && live_admission.as_ref().is_some_and(|lease| {
                            lease.target.run_id == run.run_id && !lease.expired_at(now)
                        })
                });
                if consumer_has_live_admission {
                    continue;
                }
                if consumer
                    .as_ref()
                    .is_some_and(|run| run.status == RunStatus::Completed)
                {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Delivered;
                    record
                        .continuation_run_id
                        .clone_from(&claim.continuation_run_id);
                    record.delivered_claim_id = Some(claim.claim_id);
                    record.automatic_continuation_suppressed_by_run_id = None;
                } else {
                    record.delivery_status = DurableBackgroundSubagentDeliveryStatus::Undelivered;
                    if let Some(run) = consumer.as_ref()
                        && matches!(run.status, RunStatus::Failed | RunStatus::Cancelled)
                    {
                        record.automatic_continuation_suppressed_by_run_id =
                            Some(run.run_id.clone());
                    }
                }
                record.delivery_claim = None;
                record.updated_at = now;
                save_background_record(&transaction, &record, false)?;
                changed.push(record);
            }
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(changed)
    }
}

fn continuation_artifact_content(
    transaction: &Transaction<'_>,
    background: &BackgroundSubagentRecord,
    now: DateTime<Utc>,
) -> SessionStoreResult<Option<String>> {
    if background.retention_status
        != starweaver_session::DurableBackgroundSubagentRetentionStatus::Artifact
    {
        return Ok(None);
    }
    let result_ref = background.result_ref.as_ref().ok_or_else(|| {
        SessionStoreError::Conflict("artifact result is missing terminal evidence".to_string())
    })?;
    let artifact_ref = result_ref.artifact_ref.as_deref().ok_or_else(|| {
        SessionStoreError::Conflict("artifact result is missing its reference".to_string())
    })?;
    let payload = transaction
        .query_row(
            "SELECT artifact FROM background_subagent_artifacts WHERE artifact_ref = ?1",
            params![artifact_ref],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(artifact_ref.to_string()))?;
    let artifact = deserialize_json_record::<BackgroundSubagentArtifact>(&payload)?;
    if !artifact.is_available_at(now)
        || artifact.namespace_id != background.namespace_id
        || artifact.attempt_id != background.attempt_id
        || artifact.digest != result_ref.digest.clone().unwrap_or_default()
        || artifact.size_bytes != result_ref.size_bytes
        || background.retention_expires_at != Some(artifact.expires_at)
    {
        return Err(SessionStoreError::Conflict(
            "background-subagent artifact failed integrity or retention validation".to_string(),
        ));
    }
    Ok(Some(artifact.content))
}

fn continuation_receipt_matches_request(
    background: &BackgroundSubagentRecord,
    request: &AcquireBackgroundSubagentContinuation,
    admission: &RunAdmissionReceipt,
) -> bool {
    let proposed = &request.admission.run;
    let admitted = &admission.run;
    background.validates_continuation_cause_envelope(&request.cause, admitted)
        && admitted.session_id == proposed.session_id
        && admitted.run_id == proposed.run_id
        && admitted.conversation_id == proposed.conversation_id
        && admitted.input == proposed.input
        && admitted.parent_run_id == proposed.parent_run_id
        && admitted.parent_task_id == proposed.parent_task_id
        && admitted.trigger_type == proposed.trigger_type
        && admitted.profile == proposed.profile
        && admitted.trace_context == proposed.trace_context
        && admitted.metadata == proposed.metadata
        && admission.lease.target.session_id == proposed.session_id
        && admission.lease.target.run_id == proposed.run_id
        && admission.lease.target.namespace_id == request.admission.namespace_id
        && admission.lease.command_fingerprint == request.admission.command_fingerprint
        && admission.lease.idempotency_key == request.admission.idempotency_key
}

fn background_terminal_fingerprint(
    record: &BackgroundSubagentRecord,
    artifact: Option<&BackgroundSubagentArtifact>,
) -> SessionStoreResult<String> {
    BackgroundSubagentTerminalCommit {
        record: record.clone(),
        artifact: artifact.cloned(),
        artifact_limits: None,
    }
    .canonical_fingerprint()
    .map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn persisted_background_terminal_fingerprint(
    transaction: &Transaction<'_>,
    current: &BackgroundSubagentRecord,
) -> SessionStoreResult<Option<String>> {
    let fingerprint = transaction
        .query_row(
            "SELECT terminal_fingerprint FROM background_subagent_records WHERE attempt_id = ?1",
            params![current.attempt_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(map_sqlite_session_error)?;
    if fingerprint.is_some() {
        return Ok(fingerprint);
    }
    reconstruct_background_terminal_fingerprint(transaction, current)
}

fn reconstruct_background_terminal_fingerprint(
    transaction: &Transaction<'_>,
    current: &BackgroundSubagentRecord,
) -> SessionStoreResult<Option<String>> {
    if !current.execution_status.is_terminal()
        || current.retention_status
            == starweaver_session::DurableBackgroundSubagentRetentionStatus::Expired
        || current.result_ref.is_none()
    {
        return Ok(None);
    }
    let Some(terminal_at) = current.terminal_at else {
        return Ok(None);
    };
    if current
        .retention_expires_at
        .is_none_or(|deadline| deadline <= terminal_at)
    {
        return Ok(None);
    }
    let artifact = match current.retention_status {
        starweaver_session::DurableBackgroundSubagentRetentionStatus::Inline => None,
        starweaver_session::DurableBackgroundSubagentRetentionStatus::Artifact => {
            let Some(result_ref) = current.result_ref.as_ref() else {
                return Ok(None);
            };
            let Some(artifact_ref) = result_ref.artifact_ref.as_deref() else {
                return Ok(None);
            };
            let Some(artifact) = transaction
                .query_row(
                    "SELECT artifact FROM background_subagent_artifacts WHERE artifact_ref = ?1",
                    params![artifact_ref],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?
                .map(|payload| deserialize_json_record::<BackgroundSubagentArtifact>(&payload))
                .transpose()?
            else {
                return Ok(None);
            };
            if !artifact.is_valid()
                || artifact.namespace_id != current.namespace_id
                || artifact.attempt_id != current.attempt_id
                || artifact.digest != result_ref.digest.clone().unwrap_or_default()
                || artifact.size_bytes != result_ref.size_bytes
                || current.retention_expires_at != Some(artifact.expires_at)
            {
                return Ok(None);
            }
            Some(artifact)
        }
        starweaver_session::DurableBackgroundSubagentRetentionStatus::Expired => return Ok(None),
    };
    let mut terminal = current.clone();
    terminal.updated_at = terminal_at;
    background_terminal_fingerprint(&terminal, artifact.as_ref()).map(Some)
}

fn save_background_terminal_fingerprint(
    transaction: &Transaction<'_>,
    attempt_id: &SubagentAttemptId,
    fingerprint: &str,
) -> SessionStoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE background_subagent_records SET terminal_fingerprint = ?2 WHERE attempt_id = ?1",
            params![attempt_id.as_str(), fingerprint],
        )
        .map_err(map_sqlite_session_error)?;
    if changed != 1 {
        return Err(SessionStoreError::NotFound(attempt_id.as_str().to_string()));
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum BackgroundClaimConsumerState {
    Reclaimable,
    Live,
    Completed(starweaver_core::RunId),
    Terminated(starweaver_core::RunId),
}

fn background_claim_consumer_state(
    transaction: &Transaction<'_>,
    record: &BackgroundSubagentRecord,
    claim: &DurableBackgroundSubagentDeliveryClaim,
    now: DateTime<Utc>,
) -> SessionStoreResult<BackgroundClaimConsumerState> {
    let Some(run_id) = claim.continuation_run_id.as_ref() else {
        return Ok(BackgroundClaimConsumerState::Reclaimable);
    };
    let run = transaction
        .query_row(
            "SELECT record FROM run_records WHERE session_id = ?1 AND run_id = ?2",
            params![record.parent_session_id.as_str(), run_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .map(|payload| deserialize_json_record::<RunRecord>(&payload))
        .transpose()?;
    let Some(run) = run else {
        return Ok(BackgroundClaimConsumerState::Reclaimable);
    };
    let state = match run.status {
        RunStatus::Completed => BackgroundClaimConsumerState::Completed(run_id.clone()),
        RunStatus::Failed | RunStatus::Cancelled => {
            BackgroundClaimConsumerState::Terminated(run_id.clone())
        }
        status
            if status.is_active()
                && load_session_admission(
                    transaction,
                    &record.namespace_id,
                    &record.parent_session_id,
                )?
                .is_some_and(|lease| lease.target.run_id == *run_id && !lease.expired_at(now)) =>
        {
            BackgroundClaimConsumerState::Live
        }
        _ => BackgroundClaimConsumerState::Reclaimable,
    };
    Ok(state)
}

fn commit_background_terminal_transaction(
    transaction: &Transaction<'_>,
    mut record: BackgroundSubagentRecord,
    artifact: Option<BackgroundSubagentArtifact>,
    artifact_limits: Option<BackgroundSubagentArtifactLimits>,
) -> SessionStoreResult<BackgroundSubagentRecord> {
    if !record.is_valid_terminal() {
        return Err(SessionStoreError::Conflict(
            "invalid background-subagent terminal record".to_string(),
        ));
    }
    if artifact.is_some() != artifact_limits.is_some()
        || artifact_limits.is_some_and(|limits| !limits.is_valid())
    {
        return Err(SessionStoreError::Conflict(
            "background-subagent artifact requires valid host quota limits".to_string(),
        ));
    }
    let current = load_background_record(transaction, &record.attempt_id)?;
    ensure_background_parent_writable(transaction, &current)?;
    let terminal_fingerprint = background_terminal_fingerprint(&record, artifact.as_ref())?;
    if current.execution_status.is_terminal() {
        let persisted_fingerprint =
            persisted_background_terminal_fingerprint(transaction, &current)?;
        return if persisted_fingerprint.as_deref() == Some(terminal_fingerprint.as_str()) {
            save_background_terminal_fingerprint(
                transaction,
                &current.attempt_id,
                &terminal_fingerprint,
            )?;
            Ok(current)
        } else {
            Err(SessionStoreError::Conflict(format!(
                "terminal background attempt {} is immutable",
                record.attempt_id.as_str()
            )))
        };
    }
    let now = Utc::now();
    if current.owner_lease.expired_at(now) {
        return Err(SessionStoreError::Conflict(format!(
            "background owner lease expired for {}",
            record.attempt_id.as_str()
        )));
    }
    if let (Some(artifact), Some(limits)) = (artifact.as_ref(), artifact_limits) {
        if !artifact.is_available_at(now) {
            return Err(SessionStoreError::Conflict(
                "background-subagent artifact is invalid or already expired".to_string(),
            ));
        }
        let retained_bytes = {
            let mut statement = transaction
                .prepare(
                    "SELECT artifact FROM background_subagent_artifacts
                     WHERE namespace_id = ?1 AND artifact_ref != ?2 AND expires_at > ?3",
                )
                .map_err(map_sqlite_session_error)?;
            let payloads = statement
                .query_map(
                    params![
                        artifact.namespace_id,
                        artifact.artifact_ref,
                        now.to_rfc3339()
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?;
            payloads
                .into_iter()
                .map(|payload| deserialize_json_record::<BackgroundSubagentArtifact>(&payload))
                .collect::<SessionStoreResult<Vec<_>>>()?
                .into_iter()
                .map(|current| current.size_bytes)
                .fold(0u64, u64::saturating_add)
        };
        if artifact.size_bytes > limits.max_single_bytes
            || retained_bytes.saturating_add(artifact.size_bytes) > limits.max_retained_bytes
        {
            return Err(SessionStoreError::QuotaExceeded(
                "background-subagent artifact exceeds host retention quota".to_string(),
            ));
        }
    }
    if !same_background_identity(&current, &record)
        || !same_background_owner(&current, &record)
        || !valid_background_terminal_base(&current, &record)
        || !valid_background_transition(current.execution_status, record.execution_status)
    {
        return Err(SessionStoreError::Conflict(format!(
            "invalid terminal background transition for {}",
            record.attempt_id.as_str()
        )));
    }
    if let Some(artifact) = artifact {
        let artifact_matches = artifact.is_available_at(now)
            && artifact.attempt_id == record.attempt_id
            && artifact.namespace_id == record.namespace_id
            && record.retention_status
                == starweaver_session::DurableBackgroundSubagentRetentionStatus::Artifact
            && record.retention_expires_at == Some(artifact.expires_at)
            && record.result_ref.as_ref().is_some_and(|result| {
                result.artifact_ref.as_deref() == Some(artifact.artifact_ref.as_str())
                    && result.digest.as_deref() == Some(artifact.digest.as_str())
                    && result.size_bytes == artifact.size_bytes
            });
        if !artifact_matches {
            return Err(SessionStoreError::Conflict(
                "background-subagent artifact does not match terminal evidence".to_string(),
            ));
        }
        let existing = transaction
            .query_row(
                "SELECT artifact FROM background_subagent_artifacts WHERE artifact_ref = ?1",
                params![artifact.artifact_ref],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        if let Some(existing) = existing {
            let existing = deserialize_json_record::<BackgroundSubagentArtifact>(&existing)?;
            if existing != artifact {
                return Err(SessionStoreError::Conflict(
                    "background-subagent artifact identity conflict".to_string(),
                ));
            }
        } else {
            let payload = serialize_json_record(&artifact)?;
            transaction
                .execute(
                    "INSERT INTO background_subagent_artifacts
                     (artifact_ref, namespace_id, attempt_id, expires_at, artifact)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        artifact.artifact_ref,
                        artifact.namespace_id,
                        artifact.attempt_id.as_str(),
                        artifact.expires_at.to_rfc3339(),
                        payload
                    ],
                )
                .map_err(map_sqlite_session_error)?;
        }
    } else if record.retention_status
        == starweaver_session::DurableBackgroundSubagentRetentionStatus::Artifact
    {
        return Err(SessionStoreError::Conflict(
            "artifact retention requires an atomic artifact payload".to_string(),
        ));
    }
    record.owner_lease = current.owner_lease;
    save_background_record(transaction, &record, false)?;
    save_background_terminal_fingerprint(transaction, &record.attempt_id, &terminal_fingerprint)?;
    Ok(record)
}

fn ensure_background_parent_writable(
    transaction: &Transaction<'_>,
    record: &BackgroundSubagentRecord,
) -> SessionStoreResult<()> {
    let session = load_session_record(transaction, &record.parent_session_id)?;
    if session.namespace_id != record.namespace_id
        || session.status != SessionStatus::Active
        || session.deletion_fence.blocks_continuation()
    {
        return Err(SessionStoreError::Conflict(
            "background owner write rejected because parent session is deleting or deleted"
                .to_string(),
        ));
    }
    Ok(())
}

fn load_background_record(
    connection: &rusqlite::Connection,
    attempt_id: &SubagentAttemptId,
) -> SessionStoreResult<BackgroundSubagentRecord> {
    load_background_record_optional(connection, attempt_id)?
        .ok_or_else(|| SessionStoreError::NotFound(attempt_id.as_str().to_string()))
}

fn load_background_record_optional(
    connection: &rusqlite::Connection,
    attempt_id: &SubagentAttemptId,
) -> SessionStoreResult<Option<BackgroundSubagentRecord>> {
    connection
        .query_row(
            "SELECT record FROM background_subagent_records WHERE attempt_id = ?1",
            params![attempt_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .map(|payload| deserialize_json_record(&payload))
        .transpose()
}

fn save_background_record(
    transaction: &Transaction<'_>,
    record: &BackgroundSubagentRecord,
    insert: bool,
) -> SessionStoreResult<()> {
    let claim_deadline = record
        .delivery_claim
        .as_ref()
        .map(|claim| claim.deadline.to_rfc3339());
    let execution_status = execution_status_name(record.execution_status);
    let delivery_status = delivery_status_name(record.delivery_status);
    let retention_status = record.retention_status.as_str();
    let payload = serialize_json_record(record)?;
    if insert {
        transaction
            .execute(
                "INSERT INTO background_subagent_records
                 (attempt_id, namespace_id, parent_session_id, parent_run_id, agent_id,
                  execution_status, delivery_status, retention_status, claim_deadline,
                  continuation_run_id, owner_host_instance_id, owner_generation,
                  owner_heartbeat_at, owner_lease_expires_at, retention_expires_at,
                  record, accepted_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                params![
                    record.attempt_id.as_str(),
                    record.namespace_id,
                    record.parent_session_id.as_str(),
                    record.parent_run_id.as_str(),
                    record.agent_id,
                    execution_status,
                    delivery_status,
                    retention_status,
                    claim_deadline,
                    record
                        .continuation_run_id
                        .as_ref()
                        .map(starweaver_core::RunId::as_str),
                    record.owner_lease.host_instance_id,
                    i64::try_from(record.owner_lease.fencing_generation).unwrap_or(i64::MAX),
                    record.owner_lease.heartbeat_at.to_rfc3339(),
                    record.owner_lease.lease_expires_at.to_rfc3339(),
                    record.retention_expires_at.map(|value| value.to_rfc3339()),
                    payload,
                    record.accepted_at.to_rfc3339(),
                    record.updated_at.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
    } else {
        let changed = transaction
            .execute(
                "UPDATE background_subagent_records SET execution_status = ?2,
                 delivery_status = ?3, retention_status = ?4, claim_deadline = ?5,
                 continuation_run_id = ?6, owner_heartbeat_at = ?7,
                 owner_lease_expires_at = ?8, retention_expires_at = ?9,
                 record = ?10, updated_at = ?11 WHERE attempt_id = ?1",
                params![
                    record.attempt_id.as_str(),
                    execution_status,
                    delivery_status,
                    retention_status,
                    claim_deadline,
                    record
                        .continuation_run_id
                        .as_ref()
                        .map(starweaver_core::RunId::as_str),
                    record.owner_lease.heartbeat_at.to_rfc3339(),
                    record.owner_lease.lease_expires_at.to_rfc3339(),
                    record.retention_expires_at.map(|value| value.to_rfc3339()),
                    payload,
                    record.updated_at.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
        if changed != 1 {
            return Err(SessionStoreError::NotFound(
                record.attempt_id.as_str().to_string(),
            ));
        }
    }
    Ok(())
}

fn load_run_admission_receipt(
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

fn same_background_identity(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.schema_version == next.schema_version
        && current.attempt_id == next.attempt_id
        && current.agent_id == next.agent_id
        && current.linked_task_id == next.linked_task_id
        && current.subagent_name == next.subagent_name
        && current.namespace_id == next.namespace_id
        && current.parent_session_id == next.parent_session_id
        && current.parent_run_id == next.parent_run_id
        && current.profile == next.profile
        && current.owner_lease.host_instance_id == next.owner_lease.host_instance_id
        && current.owner_lease.fencing_generation == next.owner_lease.fencing_generation
        && current.accepted_at == next.accepted_at
}

fn same_background_owner(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.owner_lease.host_instance_id == next.owner_lease.host_instance_id
        && current.owner_lease.fencing_generation == next.owner_lease.fencing_generation
}

fn valid_background_terminal_base(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.delivery_status == DurableBackgroundSubagentDeliveryStatus::Undelivered
        && current.delivery_claim.is_none()
        && current.delivered_claim_id.is_none()
        && current.continuation_run_id.is_none()
        && current
            .automatic_continuation_suppressed_by_run_id
            .is_none()
        && next.automatic_continuation_suppressed_by_run_id.is_none()
        && current.retention_status
            == starweaver_session::DurableBackgroundSubagentRetentionStatus::Inline
        && current.retention_expires_at.is_none()
        && matches!(
            next.retention_status,
            starweaver_session::DurableBackgroundSubagentRetentionStatus::Inline
                | starweaver_session::DurableBackgroundSubagentRetentionStatus::Artifact
        )
        && current.trace_context == next.trace_context
        && next.updated_at >= current.updated_at
}

fn same_background_non_execution_state(
    current: &BackgroundSubagentRecord,
    next: &BackgroundSubagentRecord,
) -> bool {
    current.continuation_run_id == next.continuation_run_id
        && current.result_ref == next.result_ref
        && current.failure_category == next.failure_category
        && current.cancellation_reason == next.cancellation_reason
        && current.delivery_status == next.delivery_status
        && current.delivery_claim == next.delivery_claim
        && current.delivered_claim_id == next.delivered_claim_id
        && current.automatic_continuation_suppressed_by_run_id
            == next.automatic_continuation_suppressed_by_run_id
        && current.retention_status == next.retention_status
        && current.retention_expires_at == next.retention_expires_at
        && current.trace_context == next.trace_context
        && current.terminal_at == next.terminal_at
        && next.updated_at >= current.updated_at
}

fn valid_background_transition(
    current: DurableBackgroundSubagentExecutionStatus,
    next: DurableBackgroundSubagentExecutionStatus,
) -> bool {
    current == next
        || matches!(
            (current, next),
            (
                DurableBackgroundSubagentExecutionStatus::Accepted,
                DurableBackgroundSubagentExecutionStatus::Starting
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Starting,
                DurableBackgroundSubagentExecutionStatus::Running
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Running,
                DurableBackgroundSubagentExecutionStatus::Waiting
                    | DurableBackgroundSubagentExecutionStatus::Completed
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            ) | (
                DurableBackgroundSubagentExecutionStatus::Waiting,
                DurableBackgroundSubagentExecutionStatus::Running
                    | DurableBackgroundSubagentExecutionStatus::Completed
                    | DurableBackgroundSubagentExecutionStatus::Failed
                    | DurableBackgroundSubagentExecutionStatus::Cancelled
            )
        )
}

const fn execution_status_name(status: DurableBackgroundSubagentExecutionStatus) -> &'static str {
    match status {
        DurableBackgroundSubagentExecutionStatus::Accepted => "accepted",
        DurableBackgroundSubagentExecutionStatus::Starting => "starting",
        DurableBackgroundSubagentExecutionStatus::Running => "running",
        DurableBackgroundSubagentExecutionStatus::Waiting => "waiting",
        DurableBackgroundSubagentExecutionStatus::Completed => "completed",
        DurableBackgroundSubagentExecutionStatus::Failed => "failed",
        DurableBackgroundSubagentExecutionStatus::Cancelled => "cancelled",
    }
}

const fn delivery_status_name(status: DurableBackgroundSubagentDeliveryStatus) -> &'static str {
    match status {
        DurableBackgroundSubagentDeliveryStatus::Undelivered => "undelivered",
        DurableBackgroundSubagentDeliveryStatus::Claimed => "claimed",
        DurableBackgroundSubagentDeliveryStatus::Delivered => "delivered",
    }
}
