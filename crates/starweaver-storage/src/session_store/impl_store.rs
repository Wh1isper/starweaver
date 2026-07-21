use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{RunId, RunLifecycle, SessionId, SubagentAttemptId};
use starweaver_session::{
    AcquireBackgroundSubagentContinuation, AcquireRunAdmission, ApprovalRecord,
    BackgroundSubagentArtifact, BackgroundSubagentContinuationReceipt, BackgroundSubagentRecord,
    BackgroundSubagentTerminalCommit, CompactRunTrace, CompactSessionTrace, DeferredToolRecord,
    DurableBackgroundSubagentDeliveryClaim, DurableBackgroundSubagentDeliveryRelease,
    DurableControlReceipt, EnvironmentStateRef, HitlResumeAbortOutcome, HitlResumeClaim,
    HitlResumeClaimState, ManagedRunTarget, PendingStreamPublication, RunAdmissionLease,
    RunAdmissionReceipt, RunEvidenceCommit, RunRecord, RunStatus, RunTerminalError,
    RunTerminalProjection, SessionContinuationFence, SessionFilter, SessionRecord,
    SessionResumeSnapshot, SessionStatus, SessionStore, SessionStoreError, SessionStoreResult,
    StreamCursorRef, StreamPublicationTarget, UpdateManagedSession,
};
use starweaver_stream::{
    AgentStreamRecord, InMemoryReplayEventLog, ReplayCursor, ReplayEvent, ReplayScope,
};

use crate::{
    domain::insert_exact_replay_event,
    sqlite::{
        collect_json_record_rows, deserialize_json_record, format_run_key,
        map_display_session_error, map_sqlite_session_error, serialize_json_record,
    },
};

use super::{
    SqliteSessionStore,
    management::ensure_run_admission_in_transaction,
    records::{
        allocate_or_reuse_run_sequence, apply_run_to_session, list_run_records, load_run_record,
        load_session_record, save_run_record, save_session_record,
    },
    trace_helpers::{
        count_deferred_tools, count_pending_approvals, latest_stream_sequence, load_checkpoint_ids,
    },
};

const fn checkpoint_run_status(status: RunLifecycle) -> RunStatus {
    match status {
        RunLifecycle::Starting | RunLifecycle::Running => RunStatus::Running,
        RunLifecycle::Waiting => RunStatus::Waiting,
        RunLifecycle::Completed => RunStatus::Completed,
        RunLifecycle::Failed => RunStatus::Failed,
        RunLifecycle::Cancelled => RunStatus::Cancelled,
    }
}

fn checkpoint_terminal_error(status: RunLifecycle) -> Option<RunTerminalError> {
    match status {
        RunLifecycle::Failed => Some(RunTerminalError::new(
            "checkpoint_run_failed",
            "run failed while checkpointing",
        )),
        RunLifecycle::Cancelled => Some(RunTerminalError::new(
            "checkpoint_run_cancelled",
            "run was cancelled while checkpointing",
        )),
        RunLifecycle::Starting
        | RunLifecycle::Running
        | RunLifecycle::Waiting
        | RunLifecycle::Completed => None,
    }
}

impl SqliteSessionStore {
    /// Atomically allocate or preserve a run sequence and persist the run.
    ///
    /// A zero sequence requests the next session-local sequence for a new run. Updating an
    /// existing run reuses its persisted sequence, and an explicit sequence change is rejected.
    ///
    /// # Errors
    ///
    /// Returns a store error for missing sessions, immutable-sequence violations, sequence
    /// collisions, serialization failures, or SQLite failures.
    pub fn append_run_allocated(&self, mut run: RunRecord) -> SessionStoreResult<RunRecord> {
        run.validate_new_write().map_err(|error| {
            SessionStoreError::Failed(format!(
                "invalid run state for {}: {error}",
                run.run_id.as_str()
            ))
        })?;
        run.updated_at = Utc::now();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, &run.session_id)?;
        allocate_or_reuse_run_sequence(&transaction, &mut run)?;
        save_run_record(&transaction, &run)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(run)
    }

    fn commit_checkpoint_sync(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
        admission_lease: Option<&RunAdmissionLease>,
    ) -> SessionStoreResult<()> {
        if let Some(lease) = admission_lease
            && (lease.target.session_id != *session_id || lease.target.run_id != checkpoint.run_id)
        {
            return Err(SessionStoreError::Conflict(
                "checkpoint does not match admission target".to_string(),
            ));
        }
        let created_at = Utc::now();
        let payload = serialize_json_record(&checkpoint)?;
        let sequence = i64::try_from(checkpoint.run_step).map_err(map_display_session_error)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = match load_session_record(&transaction, session_id) {
            Ok(session) => session,
            Err(SessionStoreError::NotFound(_)) => SessionRecord::new(session_id.clone()),
            Err(error) => return Err(error),
        };
        let mut run = match load_run_record(&transaction, session_id, &checkpoint.run_id) {
            Ok(run) => run,
            Err(SessionStoreError::NotFound(_)) => {
                let mut run = RunRecord::new(
                    session_id.clone(),
                    checkpoint.run_id.clone(),
                    checkpoint.conversation_id.clone(),
                );
                run.status = checkpoint_run_status(checkpoint.resume.status);
                run.terminal_error = checkpoint_terminal_error(checkpoint.resume.status);
                run.trace_context = checkpoint.resume.trace_context.clone();
                run.parent_run_id
                    .clone_from(&checkpoint.state.parent_run_id);
                run.parent_task_id
                    .clone_from(&checkpoint.state.parent_task_id);
                allocate_or_reuse_run_sequence(&transaction, &mut run)?;
                save_run_record(&transaction, &run)?;
                run
            }
            Err(error) => return Err(error),
        };
        let inserted = transaction
            .execute(
                "INSERT OR IGNORE INTO checkpoint_records
                 (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    session_id.as_str(),
                    checkpoint.run_id.as_str(),
                    sequence,
                    checkpoint.checkpoint_id.as_str(),
                    payload,
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
        if inserted == 0 {
            let persisted = transaction
                .query_row(
                    "SELECT record FROM checkpoint_records
                     WHERE session_id = ?1 AND run_id = ?2 AND checkpoint_id = ?3",
                    params![
                        session_id.as_str(),
                        checkpoint.run_id.as_str(),
                        checkpoint.checkpoint_id.as_str(),
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(map_sqlite_session_error)?;
            if deserialize_json_record::<AgentCheckpoint>(&persisted)? != checkpoint {
                return Err(SessionStoreError::Failed(format!(
                    "checkpoint conflict for session {} run {} at sequence {sequence} checkpoint {}",
                    session_id.as_str(),
                    checkpoint.run_id.as_str(),
                    checkpoint.checkpoint_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(());
        }
        if let Some(lease) = admission_lease {
            ensure_run_admission_in_transaction(&transaction, lease, Utc::now())?;
        }
        run.latest_checkpoint = Some(starweaver_session::CheckpointRef {
            checkpoint_id: checkpoint.checkpoint_id,
            run_id: checkpoint.run_id,
            sequence: checkpoint.run_step,
            node: format!("{:?}", checkpoint.node),
            storage_ref: None,
            stream_cursor: checkpoint.resume.cursor.stream_cursor,
            created_at,
            metadata: checkpoint.metadata,
        });
        run.updated_at = created_at;
        save_run_record(&transaction, &run)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)
    }

    fn append_replay_events_fenced_sync(
        &self,
        lease: &RunAdmissionLease,
        events: Vec<ReplayEvent>,
    ) -> SessionStoreResult<()> {
        let expected_scope = ReplayScope::run(lease.target.run_id.as_str());
        let encoded = events
            .into_iter()
            .map(|event| {
                if event.scope != expected_scope {
                    return Err(SessionStoreError::Conflict(format!(
                        "replay event scope {} does not match admission run {}",
                        event.scope.as_str(),
                        lease.target.run_id.as_str()
                    )));
                }
                Ok((
                    i64::try_from(event.sequence).map_err(map_display_session_error)?,
                    serialize_json_record(&event)?,
                    event.timestamp.to_rfc3339(),
                    event,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;

        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        ensure_run_admission_in_transaction(&transaction, lease, Utc::now())?;
        for (sequence, payload, created_at, event) in encoded {
            insert_exact_replay_event(
                &transaction,
                "replay_events",
                "replay",
                &expected_scope,
                sequence,
                &payload,
                &event,
                &created_at,
            )?;
        }
        transaction.commit().map_err(map_sqlite_session_error)
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn commit_run_evidence(
        &self,
        commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        let store = self.clone();
        crate::blocking::run(move || {
            crate::SqliteStorage {
                connection: store.connection.clone(),
                live_replay: InMemoryReplayEventLog::new(),
            }
            .commit_run_evidence(commit)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn commit_run_evidence_fenced(
        &self,
        lease: &RunAdmissionLease,
        commit: RunEvidenceCommit,
    ) -> SessionStoreResult<RunRecord> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || {
            crate::SqliteStorage {
                connection: store.connection.clone(),
                live_replay: InMemoryReplayEventLog::new(),
            }
            .commit_run_evidence_fenced(&lease, commit)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn append_replay_events_fenced(
        &self,
        lease: &RunAdmissionLease,
        events: Vec<ReplayEvent>,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || store.append_replay_events_fenced_sync(&lease, events))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn commit_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || store.commit_checkpoint_sync(&session_id, checkpoint, None))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn commit_checkpoint_fenced(
        &self,
        lease: &RunAdmissionLease,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let lease = lease.clone();
        let session_id = lease.target.session_id.clone();
        crate::blocking::run(move || {
            store.commit_checkpoint_sync(&session_id, checkpoint, Some(&lease))
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn claim_hitl_resume(&self, claim: HitlResumeClaim) -> SessionStoreResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            if !claim.is_valid_preflight() {
                return Err(SessionStoreError::Failed(
                    "invalid HITL preflight claim".to_string(),
                ));
            }
            let payload = serialize_json_record(&claim)?;
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let run = load_run_record(&transaction, &claim.session_id, &claim.run_id)?;
            if run.status != RunStatus::Waiting {
                return Err(SessionStoreError::Failed(format!(
                    "run {} is not waiting",
                    claim.run_id.as_str()
                )));
            }
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO hitl_resume_claims
                 (session_id, run_id, claim_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        claim.session_id.as_str(),
                        claim.run_id.as_str(),
                        claim.claim_id,
                        payload,
                        claim.created_at.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            if inserted == 0 {
                let persisted = transaction
                    .query_row(
                        "SELECT record FROM hitl_resume_claims
                     WHERE session_id = ?1 AND run_id = ?2",
                        params![claim.session_id.as_str(), claim.run_id.as_str()],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(map_sqlite_session_error)?;
                let persisted = deserialize_json_record::<HitlResumeClaim>(&persisted)?;
                if persisted.claim_id != claim.claim_id
                    || persisted.session_id != claim.session_id
                    || persisted.run_id != claim.run_id
                    || persisted.state != HitlResumeClaimState::Preflight
                {
                    return Err(SessionStoreError::Failed(format!(
                        "run {} already has an active resume claim",
                        claim.run_id.as_str()
                    )));
                }
            }
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn start_hitl_resume_effect(
        &self,
        lease: &RunAdmissionLease,
        source_run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let lease = lease.clone();
        let source_run_id = source_run_id.clone();
        let claim_id = claim_id.to_string();
        crate::blocking::run(move || {
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            ensure_run_admission_in_transaction(&transaction, &lease, Utc::now())?;
            let target =
                load_run_record(&transaction, &lease.target.session_id, &lease.target.run_id)?;
            if target.restore_from_run_id.as_ref() != Some(&source_run_id)
                || !target.status.is_active()
            {
                return Err(SessionStoreError::Conflict(
                    "active admission is not bound to the HITL source run".to_string(),
                ));
            }
            let source = load_run_record(&transaction, &lease.target.session_id, &source_run_id)?;
            if source.status != RunStatus::Waiting {
                return Err(SessionStoreError::Conflict(
                    "HITL source run is not waiting".to_string(),
                ));
            }
            let payload = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
                    params![lease.target.session_id.as_str(), source_run_id.as_str()],
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
            let mut claim = deserialize_json_record::<HitlResumeClaim>(&payload)?;
            if claim.claim_id != claim_id
                || claim.session_id != lease.target.session_id
                || claim.run_id != source_run_id
                || claim.state != HitlResumeClaimState::Admitted
            {
                return Err(SessionStoreError::Conflict(format!(
                    "invalid admitted resume claim for run {}",
                    source_run_id.as_str()
                )));
            }
            claim.state = HitlResumeClaimState::Started;
            let updated = transaction
                .execute(
                    "UPDATE hitl_resume_claims SET record = ?3
                     WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?4 AND record = ?5",
                    params![
                        claim.session_id.as_str(),
                        claim.run_id.as_str(),
                        serialize_json_record(&claim)?,
                        claim.claim_id,
                        payload,
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            if updated != 1 {
                return Err(SessionStoreError::Conflict(format!(
                    "admitted resume claim changed for run {}",
                    source_run_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn abort_admitted_hitl_resume(
        &self,
        lease: &RunAdmissionLease,
        source_run_id: &RunId,
        claim_id: &str,
        output_preview: &str,
    ) -> SessionStoreResult<HitlResumeAbortOutcome> {
        let store = self.clone();
        let lease = lease.clone();
        let source_run_id = source_run_id.clone();
        let claim_id = claim_id.to_string();
        let output_preview = output_preview.to_string();
        crate::blocking::run(move || {
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            ensure_run_admission_in_transaction(&transaction, &lease, Utc::now())?;
            let mut target =
                load_run_record(&transaction, &lease.target.session_id, &lease.target.run_id)?;
            if target.restore_from_run_id.as_ref() != Some(&source_run_id)
                || !target.status.is_active()
            {
                return Err(SessionStoreError::Conflict(
                    "active admission is not bound to the HITL source run".to_string(),
                ));
            }
            let source = load_run_record(&transaction, &lease.target.session_id, &source_run_id)?;
            if source.status != RunStatus::Waiting {
                return Err(SessionStoreError::Conflict(
                    "HITL source run is not waiting".to_string(),
                ));
            }
            let payload = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims WHERE session_id = ?1 AND run_id = ?2",
                    params![lease.target.session_id.as_str(), source_run_id.as_str()],
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
                || claim.session_id != lease.target.session_id
                || claim.run_id != source_run_id
            {
                return Err(SessionStoreError::Conflict(format!(
                    "invalid resume claim for run {}",
                    source_run_id.as_str()
                )));
            }
            if claim.state == HitlResumeClaimState::Started {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(HitlResumeAbortOutcome::EffectStarted);
            }
            if claim.state != HitlResumeClaimState::Admitted {
                return Err(SessionStoreError::Conflict(format!(
                    "invalid admitted resume claim for run {}",
                    source_run_id.as_str()
                )));
            }
            target.status = RunStatus::Failed;
            target.output_preview = Some(output_preview.clone());
            target.terminal_error = Some(RunTerminalError::new(
                "hitl_resume_preparation_failed",
                output_preview,
            ));
            target.updated_at = Utc::now();
            save_run_record(&transaction, &target)?;
            // This is a terminal target transition performed before the generic admission
            // finalizer. Keep the session pointer in the same transaction: the later finalizer
            // intentionally preserves an already-terminal target and otherwise leaves an active
            // pointer to a failed replacement that blocks the waiting source from being retried.
            let mut session = load_session_record(&transaction, &lease.target.session_id)?;
            apply_run_to_session(&mut session, &target);
            save_session_record(&transaction, &session)?;
            let deleted = transaction
                .execute(
                    "DELETE FROM hitl_resume_claims
                     WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?3 AND record = ?4",
                    params![
                        claim.session_id.as_str(),
                        claim.run_id.as_str(),
                        claim.claim_id,
                        payload,
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            if deleted != 1 {
                return Err(SessionStoreError::Conflict(format!(
                    "admitted resume claim changed while aborting run {}",
                    source_run_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            Ok(HitlResumeAbortOutcome::AbortedBeforeEffect)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn mark_hitl_resume_started(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        let claim_id = claim_id.to_string();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let claim_id = claim_id.as_str();
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let payload = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims
                 WHERE session_id = ?1 AND run_id = ?2",
                    params![session_id.as_str(), run_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?
                .ok_or_else(|| {
                    SessionStoreError::NotFound(format!("resume claim for {}", run_id.as_str()))
                })?;
            let mut claim = deserialize_json_record::<HitlResumeClaim>(&payload)?;
            if claim.claim_id != claim_id {
                return Err(SessionStoreError::Failed(format!(
                    "resume claim conflict for run {}",
                    run_id.as_str()
                )));
            }
            if claim.state == HitlResumeClaimState::Preflight {
                claim.state = HitlResumeClaimState::Started;
            }
            transaction
                .execute(
                    "UPDATE hitl_resume_claims SET record = ?3
                 WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?4",
                    params![
                        session_id.as_str(),
                        run_id.as_str(),
                        serialize_json_record(&claim)?,
                        claim_id
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn release_hitl_resume_claim(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        claim_id: &str,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        let claim_id = claim_id.to_string();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let claim_id = claim_id.as_str();
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let persisted = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims
                 WHERE session_id = ?1 AND run_id = ?2",
                    params![session_id.as_str(), run_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?;
            let Some(persisted) = persisted else {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(());
            };
            let persisted = deserialize_json_record::<HitlResumeClaim>(&persisted)?;
            if persisted.claim_id != claim_id {
                return Err(SessionStoreError::Failed(format!(
                    "resume claim conflict for run {}",
                    run_id.as_str()
                )));
            }
            if persisted.state != HitlResumeClaimState::Preflight {
                return Err(SessionStoreError::Failed(format!(
                    "started resume claim for run {} cannot be released",
                    run_id.as_str()
                )));
            }
            let deleted = transaction
                .execute(
                    "DELETE FROM hitl_resume_claims
                 WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?3 AND record = ?4",
                    params![
                        session_id.as_str(),
                        run_id.as_str(),
                        claim_id,
                        serialize_json_record(&persisted)?
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            if deleted != 1 {
                return Err(SessionStoreError::Failed(format!(
                    "resume claim changed while releasing run {}",
                    run_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn pending_stream_publications(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<Vec<PendingStreamPublication>> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM stream_publication_outbox
                 WHERE session_id = ?1 AND (archive_pending != 0 OR replay_pending != 0)
                 ORDER BY created_at ASC, publication_id ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows(rows)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn acknowledge_stream_publication(
        &self,
        publication_id: &str,
        target: StreamPublicationTarget,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let publication_id = publication_id.to_string();
        crate::blocking::run(move || {
            let publication_id = publication_id.as_str();
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let payload = transaction
                .query_row(
                    "SELECT record FROM stream_publication_outbox WHERE publication_id = ?1",
                    params![publication_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?;
            let Some(payload) = payload else {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(());
            };
            let mut publication = deserialize_json_record::<PendingStreamPublication>(&payload)?;
            match target {
                StreamPublicationTarget::Archive => publication.archive_pending = false,
                StreamPublicationTarget::Replay => publication.replay_pending = false,
            }
            if publication.is_complete() {
                transaction
                    .execute(
                        "DELETE FROM stream_publication_outbox WHERE publication_id = ?1",
                        params![publication_id],
                    )
                    .map_err(map_sqlite_session_error)?;
            } else {
                let payload = serialize_json_record(&publication)?;
                transaction
                    .execute(
                        "UPDATE stream_publication_outbox
                     SET record = ?2, archive_pending = ?3, replay_pending = ?4, updated_at = ?5
                     WHERE publication_id = ?1",
                        params![
                            publication_id,
                            payload,
                            i64::from(publication.archive_pending),
                            i64::from(publication.replay_pending),
                            Utc::now().to_rfc3339(),
                        ],
                    )
                    .map_err(map_sqlite_session_error)?;
            }
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn create_session_idempotent(
        &self,
        session: SessionRecord,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let store = self.clone();
        let key = idempotency_key.to_string();
        let fingerprint = command_fingerprint.to_string();
        crate::blocking::run(move || {
            store.create_session_idempotent_sync(session, &key, &fingerprint)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_session_mutation_receipt(
        &self,
        namespace_id: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<Option<SessionRecord>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        let idempotency_key = idempotency_key.to_string();
        let command_fingerprint = command_fingerprint.to_string();
        crate::blocking::run(move || {
            store.load_session_mutation_receipt_sync(
                &namespace_id,
                &idempotency_key,
                &command_fingerprint,
            )
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn update_managed_session(
        &self,
        command: UpdateManagedSession,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let store = self.clone();
        let fingerprint = command_fingerprint.to_string();
        crate::blocking::run(move || store.update_managed_session_sync(command, &fingerprint))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn acquire_session_deletion_fence(
        &self,
        session_id: &SessionId,
        expected_revision: u64,
        fence_id: &str,
        requested_by: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let store = self.clone();
        let session_id = session_id.clone();
        let fence_id = fence_id.to_string();
        let requested_by = requested_by.to_string();
        let idempotency_key = idempotency_key.to_string();
        let fingerprint = command_fingerprint.to_string();
        crate::blocking::run(move || {
            store.acquire_session_deletion_fence_sync(
                &session_id,
                expected_revision,
                &fence_id,
                &requested_by,
                &idempotency_key,
                &fingerprint,
            )
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn tombstone_session(
        &self,
        session_id: &SessionId,
        fence_id: &str,
    ) -> SessionStoreResult<SessionRecord> {
        let store = self.clone();
        let session_id = session_id.clone();
        let fence_id = fence_id.to_string();
        crate::blocking::run(move || store.tombstone_session_sync(&session_id, &fence_id))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn session_continuation_fence(
        &self,
        namespace_id: &str,
        session_id: &SessionId,
    ) -> SessionStoreResult<SessionContinuationFence> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            store.session_continuation_fence_sync(&namespace_id, &session_id)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn acquire_run_admission(
        &self,
        request: AcquireRunAdmission,
    ) -> SessionStoreResult<RunAdmissionReceipt> {
        let store = self.clone();
        crate::blocking::run(move || store.acquire_run_admission_sync(request))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn load_run_admission_receipt(
        &self,
        namespace_id: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
    ) -> SessionStoreResult<Option<RunAdmissionReceipt>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        let idempotency_key = idempotency_key.to_string();
        let command_fingerprint = command_fingerprint.to_string();
        crate::blocking::run(move || {
            store.load_run_admission_receipt_sync(
                &namespace_id,
                &idempotency_key,
                &command_fingerprint,
            )
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn heartbeat_run_admission(
        &self,
        lease: &RunAdmissionLease,
        lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<RunAdmissionLease> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || store.heartbeat_run_admission_sync(&lease, lease_expires_at))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn release_run_admission(&self, lease: &RunAdmissionLease) -> SessionStoreResult<()> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || store.release_run_admission_sync(&lease))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn update_run_status_fenced(
        &self,
        lease: &RunAdmissionLease,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<RunRecord> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || {
            store.update_run_status_fenced_sync(&lease, status, output_preview)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn finalize_run_admission(
        &self,
        lease: &RunAdmissionLease,
        terminal: RunTerminalProjection,
    ) -> SessionStoreResult<RunRecord> {
        let store = self.clone();
        let lease = lease.clone();
        crate::blocking::run(move || store.finalize_run_admission_sync(&lease, terminal))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn load_run_admission(
        &self,
        target: &ManagedRunTarget,
    ) -> SessionStoreResult<Option<RunAdmissionLease>> {
        let store = self.clone();
        let target = target.clone();
        crate::blocking::run(move || store.load_run_admission_sync(&target))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn reconcile_expired_run_admissions(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<ManagedRunTarget>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        crate::blocking::run(move || {
            store.reconcile_expired_run_admissions_sync(&namespace_id, now)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_control_receipt(
        &self,
        target: &ManagedRunTarget,
        idempotency_key: &str,
    ) -> SessionStoreResult<Option<DurableControlReceipt>> {
        let store = self.clone();
        let target = target.clone();
        let idempotency_key = idempotency_key.to_string();
        crate::blocking::run(move || store.load_control_receipt_sync(&target, &idempotency_key))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn reserve_control_receipt(
        &self,
        receipt: DurableControlReceipt,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let store = self.clone();
        crate::blocking::run(move || store.reserve_control_receipt_sync(receipt))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn update_control_receipt_state(
        &self,
        receipt_id: &str,
        state: &str,
    ) -> SessionStoreResult<DurableControlReceipt> {
        let store = self.clone();
        let receipt_id = receipt_id.to_string();
        let state = state.to_string();
        crate::blocking::run(move || store.update_control_receipt_state_sync(&receipt_id, &state))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn drain_background_subagent_operations(&self) -> SessionStoreResult<()> {
        self.background_operations.drain().await;
        Ok(())
    }

    async fn record_background_subagent_acceptance(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        self.background_operations
            .run(move || store.record_background_subagent_acceptance_sync(record))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn update_background_subagent_execution(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        self.background_operations
            .run(move || store.update_background_subagent_execution_sync(record))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn heartbeat_background_subagent(
        &self,
        attempt_id: &SubagentAttemptId,
        host_instance_id: &str,
        fencing_generation: u64,
        lease_expires_at: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        let attempt_id = attempt_id.clone();
        let host_instance_id = host_instance_id.to_string();
        self.background_operations
            .run(move || {
                store.heartbeat_background_subagent_sync(
                    &attempt_id,
                    &host_instance_id,
                    fencing_generation,
                    lease_expires_at,
                )
            })
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn commit_background_subagent_terminal(
        &self,
        commit: BackgroundSubagentTerminalCommit,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        self.background_operations
            .run(move || store.commit_background_subagent_terminal_sync(commit))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn load_background_subagent_artifact(
        &self,
        artifact_ref: &str,
    ) -> SessionStoreResult<BackgroundSubagentArtifact> {
        let store = self.clone();
        let artifact_ref = artifact_ref.to_string();
        self.background_operations
            .run(move || store.load_background_subagent_artifact_sync(&artifact_ref))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn expire_background_subagent_retention(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        self.background_operations
            .run(move || store.expire_background_subagent_retention_sync(&namespace_id, now, limit))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn record_background_subagent_terminal(
        &self,
        record: BackgroundSubagentRecord,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        self.background_operations
            .run(move || store.record_background_subagent_terminal_sync(record))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn load_background_subagent(
        &self,
        attempt_id: &SubagentAttemptId,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        let attempt_id = attempt_id.clone();
        self.background_operations
            .run(move || store.load_background_subagent_sync(&attempt_id))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn list_background_subagents(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        let session_id = session_id.cloned();
        self.background_operations
            .run(move || {
                store.list_background_subagents_sync(&namespace_id, session_id.as_ref(), limit)
            })
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn list_pending_background_subagents(
        &self,
        namespace_id: &str,
        session_id: Option<&SessionId>,
        limit: usize,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        let session_id = session_id.cloned();
        self.background_operations
            .run(move || {
                store.list_pending_background_subagents_sync(
                    &namespace_id,
                    session_id.as_ref(),
                    limit,
                )
            })
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn claim_background_subagent_delivery(
        &self,
        attempt_id: &SubagentAttemptId,
        claim: DurableBackgroundSubagentDeliveryClaim,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        let attempt_id = attempt_id.clone();
        self.background_operations
            .run(move || store.claim_background_subagent_delivery_sync(&attempt_id, claim))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn acknowledge_background_subagent_delivery(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        let attempt_id = attempt_id.clone();
        let claim_id = claim_id.to_string();
        self.background_operations
            .run(move || {
                store.acknowledge_background_subagent_delivery_sync(&attempt_id, &claim_id)
            })
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn release_background_subagent_delivery(
        &self,
        attempt_id: &SubagentAttemptId,
        claim_id: &str,
        release: DurableBackgroundSubagentDeliveryRelease,
    ) -> SessionStoreResult<BackgroundSubagentRecord> {
        let store = self.clone();
        let attempt_id = attempt_id.clone();
        let claim_id = claim_id.to_string();
        self.background_operations
            .run(move || {
                store.release_background_subagent_delivery_sync(&attempt_id, &claim_id, release)
            })
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn acquire_background_subagent_continuation(
        &self,
        request: AcquireBackgroundSubagentContinuation,
    ) -> SessionStoreResult<BackgroundSubagentContinuationReceipt> {
        let store = self.clone();
        self.background_operations
            .run(move || store.acquire_background_subagent_continuation_sync(request))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn reconcile_background_subagents(
        &self,
        namespace_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> SessionStoreResult<Vec<BackgroundSubagentRecord>> {
        let store = self.clone();
        let namespace_id = namespace_id.to_string();
        self.background_operations
            .run(move || store.reconcile_background_subagents_sync(&namespace_id, now))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn save_session(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            session.updated_at = Utc::now();
            let connection = store.lock()?;
            if let Ok(current) = load_session_record(&connection, &session.session_id) {
                session.revision = current.revision.saturating_add(1);
            }
            save_session_record(&connection, &session)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            load_session_record(&connection, session_id)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn list_sessions(&self, filter: SessionFilter) -> SessionStoreResult<Vec<SessionRecord>> {
        let store = self.clone();
        crate::blocking::run(move || {
            let connection = store.lock()?;
            let mut statement = connection
                .prepare("SELECT record FROM session_records ORDER BY updated_at DESC")
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_session_error)?;
            let mut sessions = Vec::new();
            for row in rows {
                let session = deserialize_json_record::<SessionRecord>(
                    &row.map_err(map_sqlite_session_error)?,
                )?;
                if filter.status.is_some_and(|status| session.status != status) {
                    continue;
                }
                if filter
                    .profile
                    .as_ref()
                    .is_some_and(|profile| session.profile.as_ref() != Some(profile))
                {
                    continue;
                }
                if filter
                    .workspace
                    .as_ref()
                    .is_some_and(|workspace| session.workspace.as_ref() != Some(workspace))
                {
                    continue;
                }
                sessions.push(session);
                if filter.limit.is_some_and(|limit| sessions.len() >= limit) {
                    break;
                }
            }
            Ok(sessions)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn update_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            let mut session = load_session_record(&connection, session_id)?;
            session.status = status;
            session.revision = session.revision.saturating_add(1);
            session.updated_at = Utc::now();
            save_session_record(&connection, &session)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn save_context_state(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            let mut session = load_session_record(&connection, session_id)?;
            session.state = state;
            session.revision = session.revision.saturating_add(1);
            session.updated_at = Utc::now();
            save_session_record(&connection, &session)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn save_environment_state(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            let mut session = load_session_record(&connection, session_id)?;
            session.environment_state = Some(environment_state);
            session.revision = session.revision.saturating_add(1);
            session.updated_at = Utc::now();
            save_session_record(&connection, &session)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn append_run(&self, run: RunRecord) -> SessionStoreResult<()> {
        let store = self.clone();
        crate::blocking::run(move || store.append_run_allocated(run).map(|_| ()))
            .await
            .map_err(SessionStoreError::Failed)?
    }

    async fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            load_run_record(&connection, session_id, run_id)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            list_run_records(&connection, session_id)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn update_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let mut run = load_run_record(&transaction, session_id, run_id)?;
            run.apply_legacy_status_update(status, output_preview);
            run.validate_new_write().map_err(|error| {
                SessionStoreError::Failed(format!(
                    "invalid run state for {}: {error}",
                    run.run_id.as_str()
                ))
            })?;
            run.updated_at = Utc::now();
            save_run_record(&transaction, &run)?;
            let mut session = load_session_record(&transaction, session_id)?;
            apply_run_to_session(&mut session, &run);
            save_session_record(&transaction, &session)?;
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn append_checkpoint(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let created_at = Utc::now();
            let payload = serialize_json_record(&checkpoint)?;
            let sequence = i64::try_from(checkpoint.run_step).map_err(map_display_session_error)?;
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let key = format_run_key(session_id, &checkpoint.run_id);
            let mut run = load_run_record(&transaction, session_id, &checkpoint.run_id)
                .map_err(|_| SessionStoreError::NotFound(key.clone()))?;
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO checkpoint_records
                 (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        session_id.as_str(),
                        checkpoint.run_id.as_str(),
                        sequence,
                        checkpoint.checkpoint_id.as_str(),
                        payload,
                        created_at.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            if inserted == 0 {
                let persisted = transaction
                    .query_row(
                        "SELECT record FROM checkpoint_records
                     WHERE session_id = ?1 AND run_id = ?2 AND checkpoint_id = ?3",
                        params![
                            session_id.as_str(),
                            checkpoint.run_id.as_str(),
                            checkpoint.checkpoint_id.as_str(),
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(map_sqlite_session_error)?;
                let persisted = deserialize_json_record::<AgentCheckpoint>(&persisted)?;
                if persisted != checkpoint {
                    return Err(SessionStoreError::Failed(format!(
                        "checkpoint conflict for session {} run {} at sequence {sequence} checkpoint {}",
                        session_id.as_str(),
                        checkpoint.run_id.as_str(),
                        checkpoint.checkpoint_id.as_str()
                    )));
                }
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(());
            }
            run.latest_checkpoint = Some(starweaver_session::CheckpointRef {
                checkpoint_id: checkpoint.checkpoint_id,
                run_id: checkpoint.run_id,
                sequence: checkpoint.run_step,
                node: format!("{:?}", checkpoint.node),
                storage_ref: None,
                stream_cursor: checkpoint.resume.cursor.stream_cursor,
                created_at,
                metadata: checkpoint.metadata,
            });
            run.updated_at = created_at;
            save_run_record(&transaction, &run)?;
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_checkpoints(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM checkpoint_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC, created_at ASC, rowid ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows(rows)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn latest_checkpoint(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<AgentCheckpoint>> {
        let run = self.load_run(session_id, run_id).await?;
        let Some(reference) = run.latest_checkpoint else {
            return Ok(None);
        };
        self.load_checkpoints(session_id, run_id)
            .await?
            .into_iter()
            .find(|checkpoint| checkpoint.checkpoint_id == reference.checkpoint_id)
            .map(Some)
            .ok_or_else(|| {
                SessionStoreError::NotFound(format!(
                    "checkpoint {} referenced by run {}",
                    reference.checkpoint_id.as_str(),
                    run_id.as_str()
                ))
            })
    }

    async fn append_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let records = records
                .into_iter()
                .map(|record| {
                    Ok((
                        i64::try_from(record.sequence).map_err(map_display_session_error)?,
                        serialize_json_record(&record)?,
                        record,
                    ))
                })
                .collect::<SessionStoreResult<Vec<_>>>()?;
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let mut run = load_run_record(&transaction, session_id, run_id)?;
            for (sequence, payload, record) in records {
                let inserted = transaction
                    .execute(
                        "INSERT OR IGNORE INTO stream_records
                     (session_id, run_id, sequence_no, record)
                     VALUES (?1, ?2, ?3, ?4)",
                        params![session_id.as_str(), run_id.as_str(), sequence, payload],
                    )
                    .map_err(map_sqlite_session_error)?;
                if inserted == 0 {
                    let persisted = transaction
                        .query_row(
                            "SELECT record FROM stream_records
                         WHERE session_id = ?1 AND run_id = ?2 AND sequence_no = ?3",
                            params![session_id.as_str(), run_id.as_str(), sequence],
                            |row| row.get::<_, String>(0),
                        )
                        .map_err(map_sqlite_session_error)?;
                    let persisted = deserialize_json_record::<AgentStreamRecord>(&persisted)?;
                    if persisted != record {
                        return Err(SessionStoreError::Failed(format!(
                            "stream record conflict for session {} run {} at sequence {sequence}",
                            session_id.as_str(),
                            run_id.as_str()
                        )));
                    }
                }
            }
            let latest_sequence = latest_stream_sequence(&transaction, session_id, run_id)?;
            if let Some(sequence) = latest_sequence {
                let cursor = StreamCursorRef::new(ReplayCursor::raw_runtime(
                    ReplayScope::run(run_id.as_str()),
                    sequence,
                ));
                run.stream_cursors
                    .retain(|existing| !existing.same_stream(&cursor));
                run.stream_cursors.push(cursor.clone());
                run.updated_at = Utc::now();
                save_run_record(&transaction, &run)?;
                let mut session = load_session_record(&transaction, session_id)?;
                session
                    .stream_cursors
                    .retain(|existing| !existing.same_stream(&cursor));
                session.stream_cursors.push(cursor);
                session.updated_at = run.updated_at;
                save_session_record(&transaction, &session)?;
            }
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn replay_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM stream_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows(rows)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn save_stream_cursor(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            cursor
                .validate_for_run(run_id)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            let mut connection = store.lock()?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(map_sqlite_session_error)?;
            let mut run = load_run_record(&transaction, session_id, run_id)?;
            let mut session = load_session_record(&transaction, session_id)?;
            for existing in run
                .stream_cursors
                .iter()
                .chain(session.stream_cursors.iter())
            {
                cursor
                    .validate_progression(existing)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
            }
            run.stream_cursors
                .retain(|existing| !existing.same_stream(&cursor));
            run.stream_cursors.push(cursor.clone());
            run.updated_at = Utc::now();
            save_run_record(&transaction, &run)?;

            session
                .stream_cursors
                .retain(|existing| !existing.same_stream(&cursor));
            session.stream_cursors.push(cursor);
            session.updated_at = run.updated_at;
            save_session_record(&transaction, &session)?;
            transaction.commit().map_err(map_sqlite_session_error)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn append_approval(&self, approval: ApprovalRecord) -> SessionStoreResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            let connection = store.lock()?;
            let _run = load_run_record(&connection, &approval.session_id, &approval.run_id)?;
            connection
                .execute(
                    "INSERT OR REPLACE INTO approval_records
                 (session_id, run_id, approval_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        approval.session_id.as_str(),
                        approval.run_id.as_str(),
                        approval.approval_id,
                        serialize_json_record(&approval)?,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            Ok(())
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_approvals(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM approval_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, approval_id ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows(rows)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn append_deferred_tool(&self, record: DeferredToolRecord) -> SessionStoreResult<()> {
        let store = self.clone();
        crate::blocking::run(move || {
            let connection = store.lock()?;
            let _run = load_run_record(&connection, &record.session_id, &record.run_id)?;
            connection
                .execute(
                    "INSERT OR REPLACE INTO deferred_tool_records
                 (session_id, run_id, deferred_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        record.session_id.as_str(),
                        record.run_id.as_str(),
                        record.deferred_id,
                        serialize_json_record(&record)?,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            Ok(())
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn load_deferred_tools(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            let mut statement = connection
                .prepare(
                    "SELECT record FROM deferred_tool_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY updated_at ASC, deferred_id ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str(), run_id.as_str()], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows(rows)
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<SessionResumeSnapshot> {
        let session = self.load_session(session_id).await?;
        let run = self.load_run(session_id, run_id).await?;
        let latest_checkpoint = self.latest_checkpoint(session_id, run_id).await?;
        let after_sequence = latest_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.resume.cursor.stream_cursor);
        let stream_records = self
            .replay_stream_records_after(session_id, run_id, after_sequence)
            .await?;
        let approvals = self.load_approvals(session_id, run_id).await?;
        let deferred_tools = self.load_deferred_tools(session_id, run_id).await?;
        let store = self.clone();
        let state_session_id = session_id.clone();
        let state_run_id = run_id.clone();
        let state = crate::blocking::run(move || {
            let connection = store.lock()?;
            let payload = connection
                .query_row(
                    "SELECT record FROM run_context_records
                     WHERE session_id = ?1 AND run_id = ?2",
                    params![state_session_id.as_str(), state_run_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?;
            payload
                .as_deref()
                .map(deserialize_json_record::<ResumableState>)
                .transpose()
        })
        .await
        .map_err(SessionStoreError::Failed)??
        .unwrap_or_else(|| session.state.clone());
        let environment_state = run
            .environment_state
            .clone()
            .or_else(|| session.environment_state.clone());
        let mut stream_cursors = session.stream_cursors.clone();
        stream_cursors.extend(run.stream_cursors.clone());
        Ok(SessionResumeSnapshot {
            state,
            session,
            run,
            environment_state,
            latest_checkpoint,
            stream_records,
            approvals,
            deferred_tools,
            stream_cursors,
        })
    }

    async fn compact_run_trace(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let store = self.clone();
        let session_id = session_id.clone();
        let run_id = run_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let run_id = &run_id;
            let connection = store.lock()?;
            let run = load_run_record(&connection, session_id, run_id)?;
            let checkpoints = load_checkpoint_ids(&connection, session_id, run_id)?;
            let stream_cursor = latest_stream_sequence(&connection, session_id, run_id)?;
            let approvals = count_pending_approvals(&connection, session_id, run_id)?;
            let deferred_tools = count_deferred_tools(&connection, session_id, run_id)?;
            Ok(CompactRunTrace {
                session_id: Some(session_id.clone()),
                run_id: Some(run_id.clone()),
                status: run.status,
                parent_run_id: run.parent_run_id.clone(),
                parent_task_id: run.parent_task_id.clone(),
                checkpoints: checkpoints.clone(),
                approvals,
                deferred_tools,
                latest_checkpoint: checkpoints.last().cloned(),
                stream_cursor,
                stream_cursors: run.stream_cursors.clone(),
                output_preview: run.output_preview.clone(),
                trace_context: run.trace_context.clone(),
                updated_at: Some(run.updated_at),
                metadata: run.metadata.clone(),
            })
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }

    async fn compact_session_trace(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let store = self.clone();
        let session_id = session_id.clone();
        crate::blocking::run(move || {
            let session_id = &session_id;
            let connection = store.lock()?;
            let session = load_session_record(&connection, session_id)?;
            let runs = list_run_records(&connection, session_id)?;
            let latest_run = runs.last();
            Ok(CompactSessionTrace {
                session_id: session.session_id.clone(),
                title: session.title.clone(),
                workspace: session.workspace.clone(),
                profile: session.profile.clone(),
                status: session.status,
                runs: runs.len(),
                latest_run_id: latest_run.map(|run| run.run_id.clone()),
                last_output_preview: latest_run.and_then(|run| run.output_preview.clone()),
                stream_cursors: session.stream_cursors.clone(),
                trace_context: session.trace_context.clone(),
                created_at: session.created_at,
                updated_at: session.updated_at,
                metadata: session.metadata.clone(),
            })
        })
        .await
        .map_err(SessionStoreError::Failed)?
    }
}
