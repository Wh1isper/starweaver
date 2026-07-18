//! Atomic product-neutral SQLite storage operations.

use chrono::Utc;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use starweaver_context::{AgentCheckpoint, ResumableState};
use starweaver_core::{RunId, SessionId};
use starweaver_session::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus, CheckpointRef, DeferredToolRecord,
    ExecutionStatus, HitlResumeClaim, HitlResumeClaimState, PendingStreamPublication,
    RunEvidenceCommit, RunRecord, SessionRecord, SessionStoreError, SessionStoreResult,
};
use starweaver_stream::{AgentStreamRecord, DisplayMessage, ReplayEvent, ReplayScope};

use crate::{
    SqliteStorage,
    session_store::records::{
        allocate_or_reuse_run_sequence, apply_run_to_session, list_run_records, load_run_record,
        load_session_record, save_run_record, save_session_record,
    },
    sqlite::{
        collect_json_record_rows, deserialize_json_record, map_display_session_error,
        map_sqlite_session_error, serialize_json_record, serialize_opaque_json,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceWritePoint {
    RelatedRun(usize),
    RelatedApproval { update: usize, record: usize },
    RelatedDeferredTool { update: usize, record: usize },
    ResumeClaimDelete(usize),
    PrimaryRunInitial,
    RunContext,
    RunEnvironment,
    StreamRecord(usize),
    Checkpoint(usize),
    Approval(usize),
    DeferredTool(usize),
    DisplayMessage(usize),
    ReplayEvent(usize),
    DisplaySnapshot,
    PrimaryRunFinal,
    Session,
    EvidenceDigest,
    PublicationOutbox,
    TransactionCommitted,
}

impl SqliteStorage {
    /// Create a durable session with a generated identifier.
    ///
    /// # Errors
    ///
    /// Returns a store error when the record cannot be persisted.
    pub fn create_session(
        &self,
        profile: Option<String>,
        title: Option<String>,
    ) -> SessionStoreResult<SessionRecord> {
        self.create_session_for_product(profile, title, None, None)
    }

    /// Create a durable session with product and workspace provenance.
    ///
    /// # Errors
    ///
    /// Returns a store error when the record cannot be persisted.
    pub fn create_session_for_product(
        &self,
        profile: Option<String>,
        title: Option<String>,
        workspace: Option<String>,
        source_product: Option<&str>,
    ) -> SessionStoreResult<SessionRecord> {
        let mut session = SessionRecord::new(SessionId::new());
        session.profile = profile;
        session.title = title;
        session.workspace = workspace;
        if let Some(source_product) = source_product {
            session.metadata.insert(
                crate::SESSION_SOURCE_PRODUCT_METADATA_KEY.to_string(),
                Value::String(source_product.to_string()),
            );
        }
        let connection = self.lock()?;
        save_session_record(&connection, &session)?;
        Ok(session)
    }

    /// Load a durable session record.
    ///
    /// # Errors
    ///
    /// Returns a store error when the session does not exist or cannot be decoded.
    pub fn load_session(&self, session_id: &SessionId) -> SessionStoreResult<SessionRecord> {
        let connection = self.lock()?;
        load_session_record(&connection, session_id)
    }

    /// Load a durable run record.
    ///
    /// # Errors
    ///
    /// Returns a store error when the run does not exist or cannot be decoded.
    pub fn load_run(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let connection = self.lock()?;
        load_run_record(&connection, session_id, run_id)
    }

    /// List durable sessions in most-recently-updated order.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn list_sessions(&self) -> SessionStoreResult<Vec<SessionRecord>> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare("SELECT record FROM session_records ORDER BY updated_at DESC")
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        collect_json_record_rows(rows)
    }

    /// List durable runs for one session in sequence order.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn list_runs(&self, session_id: &SessionId) -> SessionStoreResult<Vec<RunRecord>> {
        let connection = self.lock()?;
        list_run_records(&connection, session_id)
    }

    /// Replay raw runtime records for one run in sequence order.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn load_stream_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let connection = self.lock()?;
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
    }

    /// Replay display messages for one run or an entire session.
    ///
    /// Session replay flattens retained run scopes in run-sequence order before applying the
    /// optional global cursor.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn load_display_messages(
        &self,
        session_id: &SessionId,
        run_id: Option<&RunId>,
        after: Option<usize>,
    ) -> SessionStoreResult<Vec<DisplayMessage>> {
        let connection = self.lock()?;
        let payloads = if let Some(run_id) = run_id {
            let scope = ReplayScope::run(run_id.as_str());
            let after = after.map_or(-1_i64, |value| i64::try_from(value).unwrap_or(i64::MAX));
            let mut statement = connection
                .prepare(
                    "SELECT record FROM display_message_records
                     WHERE scope = ?1 AND sequence_no > ?2
                     ORDER BY sequence_no ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![scope.as_str(), after], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(map_sqlite_session_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
        } else {
            let mut statement = connection
                .prepare(
                    "SELECT events.record
                     FROM run_records AS runs
                     JOIN display_message_records AS events
                       ON events.scope = 'run:' || runs.run_id
                     WHERE runs.session_id = ?1
                     ORDER BY runs.sequence_no ASC, events.sequence_no ASC",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_session_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(map_sqlite_session_error)?
                .into_iter()
                .enumerate()
                .filter_map(|(sequence, payload)| {
                    after
                        .is_none_or(|after| sequence > after)
                        .then_some(payload)
                })
                .collect()
        };
        payloads
            .into_iter()
            .map(|payload| deserialize_json_record::<ReplayEvent>(&payload))
            .filter_map(|result| match result {
                Ok(event) => match event.event {
                    starweaver_stream::ReplayEventKind::DisplayMessage(message) => {
                        Some(Ok(*message))
                    }
                    _ => None,
                },
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    /// Atomically assign a session sequence, persist a queued run, and update session pointers.
    ///
    /// An exact retry for the same run id returns the existing run. A different payload for an
    /// existing run id is rejected.
    ///
    /// # Errors
    ///
    /// Returns a store error for missing sessions, conflicting retries, or SQLite failures.
    pub fn begin_run(&self, mut run: RunRecord) -> SessionStoreResult<RunRecord> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, &run.session_id)?;
        if let Some(persisted) = allocate_or_reuse_run_sequence(&transaction, &mut run)? {
            if run != persisted {
                return Err(SessionStoreError::Failed(format!(
                    "run conflict for session {} and run {}",
                    run.session_id.as_str(),
                    run.run_id.as_str()
                )));
            }
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(persisted);
        }
        save_run_record(&transaction, &run)?;
        apply_run_to_session(&mut session, &run);
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(run)
    }

    /// Atomically commit run/session state and all product-neutral run evidence.
    ///
    /// # Errors
    ///
    /// Returns a store error for identity mismatches, conflicting append-only evidence, missing
    /// records, serialization failures, or SQLite failures. Any failure rolls back the whole
    /// commit.
    pub fn commit_run_evidence(&self, commit: RunEvidenceCommit) -> SessionStoreResult<RunRecord> {
        self.commit_run_evidence_observed(commit, |_| Ok(()))
    }

    #[cfg(test)]
    pub(crate) fn commit_run_evidence_with_fault(
        &self,
        commit: RunEvidenceCommit,
        fail_after: EvidenceWritePoint,
    ) -> SessionStoreResult<RunRecord> {
        self.commit_run_evidence_observed(commit, |point| {
            if point == fail_after {
                return Err(SessionStoreError::Failed(format!(
                    "injected run-evidence fault after {point:?}"
                )));
            }
            Ok(())
        })
    }

    #[allow(clippy::too_many_lines)]
    fn commit_run_evidence_observed(
        &self,
        mut commit: RunEvidenceCommit,
        mut after_write: impl FnMut(EvidenceWritePoint) -> SessionStoreResult<()>,
    ) -> SessionStoreResult<RunRecord> {
        commit.run.stream_cursors.clone_from(&commit.stream_cursors);
        commit.validate()?;
        let evidence_digest = commit.digest()?;
        let publication = if commit.publication_targets.is_empty() {
            None
        } else {
            let mut publication = PendingStreamPublication::new(
                commit.run.session_id.clone(),
                commit.run.run_id.clone(),
                commit.publication_targets,
                commit.run.updated_at,
            );
            publication
                .stream_records
                .clone_from(&commit.stream_records);
            publication
                .display_messages
                .clone_from(&commit.display_messages);
            publication.replay_events.clone_from(&commit.replay_events);
            publication
                .display_snapshot
                .clone_from(&commit.display_snapshot);
            Some(publication)
        };
        let publication_payload = publication
            .as_ref()
            .map(serialize_json_record)
            .transpose()?;
        let context_payload = serialize_json_record(&commit.context_state)?;
        let environment_payload = commit
            .environment_state
            .as_ref()
            .map(serialize_opaque_json)
            .transpose()?;
        let stream_records = commit
            .stream_records
            .iter()
            .map(|record| {
                Ok((
                    i64::try_from(record.sequence).map_err(map_display_session_error)?,
                    serialize_json_record(record)?,
                    record,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let checkpoints = commit
            .checkpoints
            .iter()
            .map(|checkpoint| {
                Ok((
                    i64::try_from(checkpoint.run_step).map_err(map_display_session_error)?,
                    checkpoint.checkpoint_id.as_str().to_string(),
                    serialize_json_record(checkpoint)?,
                    checkpoint,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let approvals = commit
            .approvals
            .iter()
            .map(|record| Ok((record, serialize_json_record(record)?)))
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let deferred_tools = commit
            .deferred_tools
            .iter()
            .map(|record| Ok((record, serialize_json_record(record)?)))
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let replay_scope = ReplayScope::run(commit.run.run_id.as_str());
        let display_events = commit
            .display_messages
            .iter()
            .cloned()
            .map(|message| {
                let event = ReplayEvent::display(replay_scope.clone(), message);
                Ok((
                    i64::try_from(event.sequence).map_err(map_display_session_error)?,
                    serialize_json_record(&event)?,
                    event.timestamp.to_rfc3339(),
                    event,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let replay_events = commit
            .replay_events
            .iter()
            .map(|event| {
                Ok((
                    i64::try_from(event.sequence).map_err(map_display_session_error)?,
                    serialize_json_record(event)?,
                    event.timestamp.to_rfc3339(),
                    event,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;
        let display_snapshot = commit
            .display_snapshot
            .as_ref()
            .map(serialize_json_record)
            .transpose()?;
        let related_payloads = commit
            .related_run_updates
            .iter()
            .map(|update| {
                Ok((
                    update
                        .approvals
                        .iter()
                        .map(serialize_json_record)
                        .collect::<SessionStoreResult<Vec<_>>>()?,
                    update
                        .deferred_tools
                        .iter()
                        .map(serialize_json_record)
                        .collect::<SessionStoreResult<Vec<_>>>()?,
                ))
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;

        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, &commit.run.session_id)?;
        let existing_run = allocate_or_reuse_run_sequence(&transaction, &mut commit.run)?;
        let existing_digest = transaction
            .query_row(
                "SELECT digest FROM run_evidence_commits
                 WHERE session_id = ?1 AND run_id = ?2",
                params![commit.run.session_id.as_str(), commit.run.run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        if let Some(existing_digest) = existing_digest {
            if existing_digest == evidence_digest {
                let existing = existing_run.ok_or_else(|| {
                    SessionStoreError::Failed(
                        "evidence digest exists without run record".to_string(),
                    )
                })?;
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(existing);
            }
            return Err(SessionStoreError::Failed(format!(
                "run evidence conflict for session {} and run {}",
                commit.run.session_id.as_str(),
                commit.run.run_id.as_str()
            )));
        }
        for (update_index, (update, (approval_payloads, deferred_payloads))) in commit
            .related_run_updates
            .iter()
            .zip(&related_payloads)
            .enumerate()
        {
            let claim_id = update.resume_claim_id.as_deref().ok_or_else(|| {
                SessionStoreError::Failed(format!(
                    "related run {} requires an exclusive resume claim",
                    update.run_id.as_str()
                ))
            })?;
            let persisted_claim = transaction
                .query_row(
                    "SELECT record FROM hitl_resume_claims
                     WHERE session_id = ?1 AND run_id = ?2",
                    params![commit.run.session_id.as_str(), update.run_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(map_sqlite_session_error)?
                .ok_or_else(|| {
                    SessionStoreError::Failed(format!(
                        "related run {} has no active resume claim",
                        update.run_id.as_str()
                    ))
                })?;
            let persisted_claim = deserialize_json_record::<HitlResumeClaim>(&persisted_claim)?;
            if persisted_claim.claim_id != claim_id
                || persisted_claim.state != HitlResumeClaimState::Started
            {
                return Err(SessionStoreError::Failed(format!(
                    "started resume claim conflict for related run {}",
                    update.run_id.as_str()
                )));
            }
            let mut source = load_run_record(&transaction, &commit.run.session_id, &update.run_id)?;
            if source.status != update.expected_status {
                return Err(SessionStoreError::Failed(format!(
                    "related run {} status conflict: expected {}, found {}",
                    update.run_id.as_str(),
                    update.expected_status.as_str(),
                    source.status.as_str()
                )));
            }
            source.status = update.status;
            source.output_preview.clone_from(&update.output_preview);
            source.updated_at = commit.run.updated_at;
            save_run_record(&transaction, &source)?;
            after_write(EvidenceWritePoint::RelatedRun(update_index))?;
            apply_run_to_session(&mut session, &source);
            for (record_index, (approval, payload)) in
                update.approvals.iter().zip(approval_payloads).enumerate()
            {
                replace_resolved_approval(&transaction, approval, payload)?;
                after_write(EvidenceWritePoint::RelatedApproval {
                    update: update_index,
                    record: record_index,
                })?;
            }
            for (record_index, (deferred, payload)) in update
                .deferred_tools
                .iter()
                .zip(deferred_payloads)
                .enumerate()
            {
                replace_resolved_deferred_tool(&transaction, deferred, payload)?;
                after_write(EvidenceWritePoint::RelatedDeferredTool {
                    update: update_index,
                    record: record_index,
                })?;
            }
            transaction
                .execute(
                    "DELETE FROM hitl_resume_claims
                     WHERE session_id = ?1 AND run_id = ?2 AND claim_id = ?3",
                    params![
                        commit.run.session_id.as_str(),
                        update.run_id.as_str(),
                        claim_id
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            after_write(EvidenceWritePoint::ResumeClaimDelete(update_index))?;
        }
        validate_cursor_progression(&commit, existing_run.as_ref(), &session)?;
        save_run_record(&transaction, &commit.run)?;
        after_write(EvidenceWritePoint::PrimaryRunInitial)?;

        insert_exact_versioned_run_record(
            &transaction,
            "run_context_records",
            &commit.run.session_id,
            &commit.run.run_id,
            &context_payload,
            &commit.context_state,
            commit.run.updated_at.to_rfc3339(),
            "run context",
        )?;
        after_write(EvidenceWritePoint::RunContext)?;
        if let Some(payload) = environment_payload.as_deref() {
            insert_exact_opaque_run_record(
                &transaction,
                "run_environment_records",
                &commit.run.session_id,
                &commit.run.run_id,
                payload,
                commit.environment_state.as_ref().ok_or_else(|| {
                    SessionStoreError::Failed("missing environment state".to_string())
                })?,
                commit.run.updated_at.to_rfc3339(),
                "run environment",
            )?;
            after_write(EvidenceWritePoint::RunEnvironment)?;
        }
        for (record_index, (sequence, payload, record)) in stream_records.into_iter().enumerate() {
            insert_exact_stream_record(
                &transaction,
                &commit.run.session_id,
                &commit.run.run_id,
                sequence,
                &payload,
                record,
            )?;
            after_write(EvidenceWritePoint::StreamRecord(record_index))?;
        }
        for (record_index, (sequence, checkpoint_id, payload, checkpoint)) in
            checkpoints.iter().enumerate()
        {
            insert_exact_checkpoint(
                &transaction,
                &commit.run.session_id,
                &commit.run.run_id,
                *sequence,
                checkpoint_id,
                payload,
                checkpoint,
                commit.run.updated_at.to_rfc3339(),
            )?;
            after_write(EvidenceWritePoint::Checkpoint(record_index))?;
        }
        for (record_index, (approval, payload)) in approvals.into_iter().enumerate() {
            insert_exact_hitl_record(
                &transaction,
                "approval_records",
                "approval_id",
                &approval.session_id,
                &approval.run_id,
                &approval.approval_id,
                &payload,
                approval,
                approval.updated_at.to_rfc3339(),
                "approval",
            )?;
            after_write(EvidenceWritePoint::Approval(record_index))?;
        }
        for (record_index, (deferred, payload)) in deferred_tools.into_iter().enumerate() {
            insert_exact_hitl_record(
                &transaction,
                "deferred_tool_records",
                "deferred_id",
                &deferred.session_id,
                &deferred.run_id,
                &deferred.deferred_id,
                &payload,
                deferred,
                deferred.updated_at.to_rfc3339(),
                "deferred tool",
            )?;
            after_write(EvidenceWritePoint::DeferredTool(record_index))?;
        }
        for (record_index, (sequence, payload, created_at, event)) in
            display_events.into_iter().enumerate()
        {
            insert_exact_replay_event(
                &transaction,
                "display_message_records",
                "display",
                &replay_scope,
                sequence,
                &payload,
                &event,
                &created_at,
            )?;
            after_write(EvidenceWritePoint::DisplayMessage(record_index))?;
        }
        for (record_index, (sequence, payload, created_at, event)) in
            replay_events.into_iter().enumerate()
        {
            insert_exact_replay_event(
                &transaction,
                "replay_events",
                "replay",
                &replay_scope,
                sequence,
                &payload,
                event,
                &created_at,
            )?;
            after_write(EvidenceWritePoint::ReplayEvent(record_index))?;
        }
        if let Some(payload) = display_snapshot.as_deref() {
            insert_exact_display_snapshot(
                &transaction,
                &replay_scope,
                payload,
                commit.display_snapshot.as_ref().ok_or_else(|| {
                    SessionStoreError::Failed("missing display snapshot".to_string())
                })?,
                commit.run.updated_at.to_rfc3339(),
            )?;
            after_write(EvidenceWritePoint::DisplaySnapshot)?;
        }

        let latest_checkpoint = transaction
            .query_row(
                "SELECT record FROM checkpoint_records
                 WHERE session_id = ?1 AND run_id = ?2
                 ORDER BY sequence_no DESC, checkpoint_id DESC LIMIT 1",
                params![commit.run.session_id.as_str(), commit.run.run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?
            .map(|payload| deserialize_json_record::<AgentCheckpoint>(&payload))
            .transpose()?;
        if let Some(checkpoint) = latest_checkpoint.as_ref() {
            commit.run.latest_checkpoint = Some(checkpoint_ref(checkpoint, commit.run.updated_at));
        }
        session.state.clone_from(&commit.context_state);
        session
            .environment_state
            .clone_from(&commit.run.environment_state);
        session.stream_cursors.clone_from(&commit.stream_cursors);
        session.profile.clone_from(&commit.run.profile);
        save_run_record(&transaction, &commit.run)?;
        after_write(EvidenceWritePoint::PrimaryRunFinal)?;
        apply_run_to_session(&mut session, &commit.run);
        save_session_record(&transaction, &session)?;
        after_write(EvidenceWritePoint::Session)?;
        transaction
            .execute(
                "INSERT INTO run_evidence_commits
                 (session_id, run_id, digest, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![
                    commit.run.session_id.as_str(),
                    commit.run.run_id.as_str(),
                    evidence_digest,
                    commit.run.updated_at.to_rfc3339()
                ],
            )
            .map_err(map_sqlite_session_error)?;
        after_write(EvidenceWritePoint::EvidenceDigest)?;
        if let (Some(publication), Some(payload)) =
            (publication.as_ref(), publication_payload.as_deref())
        {
            transaction
                .execute(
                    "INSERT INTO stream_publication_outbox
                     (publication_id, session_id, run_id, record, archive_pending,
                      replay_pending, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                    params![
                        publication.publication_id,
                        publication.session_id.as_str(),
                        publication.run_id.as_str(),
                        payload,
                        i64::from(publication.archive_pending),
                        i64::from(publication.replay_pending),
                        publication.created_at.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            after_write(EvidenceWritePoint::PublicationOutbox)?;
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        after_write(EvidenceWritePoint::TransactionCommitted)?;
        Ok(commit.run)
    }

    /// Load per-run resumable context evidence.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn load_run_context(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<ResumableState>> {
        self.load_optional_run_record("run_context_records", session_id, run_id)
    }

    /// Load per-run serialized environment evidence.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn load_run_environment(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<Value>> {
        self.load_run_environment_as(session_id, run_id)
    }

    /// Deserialize a per-run provider environment snapshot into a caller-owned neutral type.
    ///
    /// The storage crate deliberately does not depend on an environment implementation crate;
    /// CLI and RPC hosts select the concrete snapshot type at their own boundary.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn load_run_environment_as<T>(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let connection = self.lock()?;
        let payload = connection
            .query_row(
                "SELECT record FROM run_environment_records WHERE session_id = ?1 AND run_id = ?2",
                params![session_id.as_str(), run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        payload
            .as_deref()
            .map(|payload| {
                serde_json::from_str(payload)
                    .map_err(|error| SessionStoreError::Failed(error.to_string()))
            })
            .transpose()
    }

    fn load_optional_run_record<T>(
        &self,
        table: &str,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Option<T>>
    where
        T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord,
    {
        let connection = self.lock()?;
        let payload = connection
            .query_row(
                &format!("SELECT record FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![session_id.as_str(), run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        payload.as_deref().map(deserialize_json_record).transpose()
    }

    /// Resolve an exact session id or unique id prefix.
    ///
    /// # Errors
    ///
    /// Returns `NotFound` for no match and a store error for ambiguous prefixes.
    pub fn resolve_session_prefix(&self, value: &str) -> SessionStoreResult<SessionId> {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT session_id FROM session_records
                 WHERE substr(session_id, 1, length(?1)) = ?1
                 ORDER BY updated_at DESC",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(params![value], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        let mut matches = Vec::new();
        for row in rows {
            matches.push(row.map_err(map_sqlite_session_error)?);
        }
        match matches.as_slice() {
            [session_id] => Ok(SessionId::from_string(session_id.clone())),
            [] => Err(SessionStoreError::NotFound(value.to_string())),
            _ => Err(SessionStoreError::Failed(format!(
                "session prefix '{value}' is ambiguous"
            ))),
        }
    }

    /// List approvals with optional session and run filters.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn list_approvals(
        &self,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        self.list_hitl_records(
            "approval_records",
            session_id.map(SessionId::as_str),
            run_id.map(RunId::as_str),
        )
    }

    /// Load an approval by globally unique id, rejecting ambiguous legacy data.
    ///
    /// # Errors
    ///
    /// Returns a store error for no match, ambiguity, SQLite, or JSON decoding failures.
    pub fn load_approval(&self, approval_id: &str) -> SessionStoreResult<ApprovalRecord> {
        self.load_unique_hitl_record("approval_records", "approval_id", approval_id)
    }

    /// Atomically decide an approval selected by globally unique id.
    ///
    /// An exact status/actor/reason retry returns the existing record. A second, different
    /// decision is rejected.
    ///
    /// # Errors
    ///
    /// Returns a store error for invalid transitions, ambiguity, missing records, or SQLite
    /// failures.
    pub fn decide_approval(
        &self,
        approval_id: &str,
        status: ApprovalStatus,
        decided_by: Option<String>,
        reason: Option<String>,
    ) -> SessionStoreResult<ApprovalRecord> {
        if status == ApprovalStatus::Pending {
            return Err(SessionStoreError::Failed(
                "approval decision must be terminal".to_string(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut approval = load_unique_hitl_record_tx::<ApprovalRecord>(
            &transaction,
            "approval_records",
            "approval_id",
            approval_id,
        )?;
        if approval.status != ApprovalStatus::Pending {
            let same = approval.status == status
                && approval.decision.as_ref().is_some_and(|decision| {
                    decision.status == status
                        && decision.decided_by == decided_by
                        && decision.reason == reason
                });
            if same {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(approval);
            }
            return Err(SessionStoreError::Failed(format!(
                "approval conflict for {approval_id}"
            )));
        }
        approval.status = status;
        approval.updated_at = Utc::now();
        approval.decision = Some(ApprovalDecision {
            status,
            decided_by,
            decided_at: approval.updated_at,
            reason,
            metadata: serde_json::Map::default(),
        });
        update_hitl_record(
            &transaction,
            "approval_records",
            "approval_id",
            &approval.approval_id,
            &serialize_json_record(&approval)?,
            approval.updated_at.to_rfc3339(),
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(approval)
    }

    /// List deferred tools with optional session and run filters.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite or JSON decoding fails.
    pub fn list_deferred_tools(
        &self,
        session_id: Option<&SessionId>,
        run_id: Option<&RunId>,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        self.list_hitl_records(
            "deferred_tool_records",
            session_id.map(SessionId::as_str),
            run_id.map(RunId::as_str),
        )
    }

    /// Load a deferred tool by globally unique id, rejecting ambiguous legacy data.
    ///
    /// # Errors
    ///
    /// Returns a store error for no match, ambiguity, SQLite, or JSON decoding failures.
    pub fn load_deferred_tool(&self, deferred_id: &str) -> SessionStoreResult<DeferredToolRecord> {
        self.load_unique_hitl_record("deferred_tool_records", "deferred_id", deferred_id)
    }

    /// Atomically resolve a deferred tool selected by globally unique id.
    ///
    /// # Errors
    ///
    /// Returns a store error for invalid transitions, conflicts, ambiguity, missing records, or
    /// SQLite failures.
    pub fn resolve_deferred_tool(
        &self,
        deferred_id: &str,
        status: ExecutionStatus,
        response: Value,
    ) -> SessionStoreResult<DeferredToolRecord> {
        if !matches!(
            status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            return Err(SessionStoreError::Failed(
                "deferred tool resolution must be terminal".to_string(),
            ));
        }
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut deferred = load_unique_hitl_record_tx::<DeferredToolRecord>(
            &transaction,
            "deferred_tool_records",
            "deferred_id",
            deferred_id,
        )?;
        if matches!(
            deferred.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            if deferred.status == status && deferred.response == response {
                transaction.commit().map_err(map_sqlite_session_error)?;
                return Ok(deferred);
            }
            return Err(SessionStoreError::Failed(format!(
                "deferred tool conflict for {deferred_id}"
            )));
        }
        deferred.status = status;
        deferred.response = response;
        deferred.updated_at = Utc::now();
        update_hitl_record(
            &transaction,
            "deferred_tool_records",
            "deferred_id",
            &deferred.deferred_id,
            &serialize_json_record(&deferred)?,
            deferred.updated_at.to_rfc3339(),
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(deferred)
    }

    /// Delete a session and all shared durable evidence in one transaction.
    ///
    /// # Errors
    ///
    /// Returns a store error when SQLite fails. Product-owned files and presentation state are
    /// intentionally outside this operation.
    pub fn delete_session(&self, session_id: &SessionId) -> SessionStoreResult<bool> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM session_records WHERE session_id = ?1)",
                params![session_id.as_str()],
                |row| row.get(0),
            )
            .map_err(map_sqlite_session_error)?;
        if !exists {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(false);
        }
        let imported_from = load_session_record(&transaction, session_id)?
            .metadata
            .get(crate::SESSION_IMPORTED_FROM_METADATA_KEY)
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let run_ids = load_run_ids(&transaction, session_id)?;
        for run_id in &run_ids {
            delete_run_evidence(&transaction, session_id, run_id)?;
        }
        transaction
            .execute(
                "DELETE FROM replay_events WHERE scope = ?1",
                params![format!("session:{}", session_id.as_str())],
            )
            .map_err(map_sqlite_session_error)?;
        transaction
            .execute(
                "DELETE FROM display_message_records WHERE scope = ?1",
                params![format!("session:{}", session_id.as_str())],
            )
            .map_err(map_sqlite_session_error)?;
        transaction
            .execute(
                "DELETE FROM display_snapshot_records WHERE scope = ?1",
                params![format!("session:{}", session_id.as_str())],
            )
            .map_err(map_sqlite_session_error)?;
        if let Some(source_path) = imported_from {
            transaction
                .execute(
                    "INSERT INTO local_store_import_tombstones
                     (source_path, session_id, deleted_at) VALUES (?1, ?2, ?3)
                     ON CONFLICT(source_path, session_id) DO UPDATE SET
                       deleted_at = excluded.deleted_at",
                    params![source_path, session_id.as_str(), Utc::now().to_rfc3339()],
                )
                .map_err(map_sqlite_session_error)?;
        }
        transaction
            .execute(
                "DELETE FROM session_records WHERE session_id = ?1",
                params![session_id.as_str()],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(true)
    }

    /// Delete selected runs and their shared durable evidence atomically.
    ///
    /// Session pointers are recomputed from retained runs. Product-owned files and presentation
    /// state remain the caller's responsibility.
    ///
    /// # Errors
    ///
    /// Returns a store error for unknown runs or SQLite failures.
    pub fn prune_runs(
        &self,
        session_id: &SessionId,
        run_ids: &[RunId],
    ) -> SessionStoreResult<usize> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let mut session = load_session_record(&transaction, session_id)?;
        for run_id in run_ids {
            let _run = load_run_record(&transaction, session_id, run_id)?;
            delete_run_evidence(&transaction, session_id, run_id)?;
        }
        let retained = list_run_records(&transaction, session_id)?;
        session.head_run_id = retained.last().map(|run| run.run_id.clone());
        session.head_success_run_id = retained
            .iter()
            .rev()
            .find(|run| run.status == starweaver_session::RunStatus::Completed)
            .map(|run| run.run_id.clone());
        session.active_run_id = retained
            .iter()
            .rev()
            .find(|run| run.status.is_active())
            .map(|run| run.run_id.clone());
        session.updated_at = Utc::now();
        save_session_record(&transaction, &session)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(run_ids.len())
    }

    fn list_hitl_records<T>(
        &self,
        table: &str,
        session_id: Option<&str>,
        run_id: Option<&str>,
    ) -> SessionStoreResult<Vec<T>>
    where
        T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord,
    {
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(&format!(
                "SELECT record FROM {table}
                 WHERE (?1 IS NULL OR session_id = ?1)
                   AND (?2 IS NULL OR run_id = ?2)
                 ORDER BY updated_at DESC"
            ))
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(params![session_id, run_id], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        collect_json_record_rows(rows)
    }

    fn load_unique_hitl_record<T>(
        &self,
        table: &str,
        id_column: &str,
        id: &str,
    ) -> SessionStoreResult<T>
    where
        T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord,
    {
        let connection = self.lock()?;
        load_unique_hitl_record_tx(&connection, table, id_column, id)
    }
}

fn validate_cursor_progression(
    commit: &RunEvidenceCommit,
    existing_run: Option<&RunRecord>,
    session: &SessionRecord,
) -> SessionStoreResult<()> {
    for cursor in &commit.stream_cursors {
        for existing in existing_run
            .into_iter()
            .flat_map(|run| run.stream_cursors.iter())
            .chain(session.stream_cursors.iter())
        {
            cursor
                .validate_progression(existing)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        }
    }
    Ok(())
}

fn checkpoint_ref(
    checkpoint: &AgentCheckpoint,
    created_at: chrono::DateTime<Utc>,
) -> CheckpointRef {
    CheckpointRef {
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        run_id: checkpoint.run_id.clone(),
        sequence: checkpoint.run_step,
        node: format!("{:?}", checkpoint.node),
        storage_ref: None,
        stream_cursor: checkpoint.resume.cursor.stream_cursor,
        created_at,
        metadata: checkpoint.metadata.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
fn insert_exact_versioned_run_record<T>(
    transaction: &Transaction<'_>,
    table: &str,
    session_id: &SessionId,
    run_id: &RunId,
    payload: &str,
    expected: &T,
    updated_at: String,
    kind: &str,
) -> SessionStoreResult<()>
where
    T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord + PartialEq,
{
    let inserted = transaction
        .execute(
            &format!(
                "INSERT OR IGNORE INTO {table} (session_id, run_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4)"
            ),
            params![session_id.as_str(), run_id.as_str(), payload, updated_at],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                &format!("SELECT record FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![session_id.as_str(), run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        if deserialize_json_record::<T>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "{kind} conflict for session {} and run {}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_exact_opaque_run_record(
    transaction: &Transaction<'_>,
    table: &str,
    session_id: &SessionId,
    run_id: &RunId,
    payload: &str,
    expected: &Value,
    updated_at: String,
    kind: &str,
) -> SessionStoreResult<()> {
    let inserted = transaction
        .execute(
            &format!(
                "INSERT OR IGNORE INTO {table} (session_id, run_id, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4)"
            ),
            params![session_id.as_str(), run_id.as_str(), payload, updated_at],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                &format!("SELECT record FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![session_id.as_str(), run_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        let persisted =
            serde_json::from_str::<Value>(&persisted).map_err(map_display_session_error)?;
        if &persisted != expected {
            return Err(SessionStoreError::Failed(format!(
                "{kind} conflict for session {} and run {}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_exact_replay_event(
    transaction: &Transaction<'_>,
    table: &str,
    family: &str,
    scope: &ReplayScope,
    sequence: i64,
    payload: &str,
    expected: &ReplayEvent,
    created_at: &str,
) -> SessionStoreResult<()> {
    let inserted = transaction
        .execute(
            &format!(
                "INSERT OR IGNORE INTO {table} (scope, sequence_no, record, created_at)\n                 VALUES (?1, ?2, ?3, ?4)"
            ),
            params![scope.as_str(), sequence, payload, created_at],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                &format!("SELECT record FROM {table} WHERE scope = ?1 AND sequence_no = ?2"),
                params![scope.as_str(), sequence],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        if deserialize_json_record::<ReplayEvent>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "{family} event conflict for scope {} at sequence {sequence}",
                scope.as_str()
            )));
        }
    }
    Ok(())
}

fn insert_exact_display_snapshot(
    transaction: &Transaction<'_>,
    scope: &ReplayScope,
    payload: &str,
    expected: &starweaver_stream::ReplaySnapshot,
    updated_at: String,
) -> SessionStoreResult<()> {
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO display_snapshot_records (scope, record, updated_at)\n             VALUES (?1, ?2, ?3)",
            params![scope.as_str(), payload, updated_at],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                "SELECT record FROM display_snapshot_records WHERE scope = ?1",
                params![scope.as_str()],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        if deserialize_json_record::<starweaver_stream::ReplaySnapshot>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "display snapshot conflict for scope {}",
                scope.as_str()
            )));
        }
    }
    Ok(())
}

fn insert_exact_stream_record(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    run_id: &RunId,
    sequence: i64,
    payload: &str,
    expected: &AgentStreamRecord,
) -> SessionStoreResult<()> {
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO stream_records (session_id, run_id, sequence_no, record)
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
        if deserialize_json_record::<AgentStreamRecord>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "stream record conflict for session {} run {} at sequence {sequence}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_exact_checkpoint(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    run_id: &RunId,
    sequence: i64,
    checkpoint_id: &str,
    payload: &str,
    expected: &AgentCheckpoint,
    created_at: String,
) -> SessionStoreResult<()> {
    let inserted = transaction
        .execute(
            "INSERT OR IGNORE INTO checkpoint_records
             (session_id, run_id, sequence_no, checkpoint_id, record, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                session_id.as_str(),
                run_id.as_str(),
                sequence,
                checkpoint_id,
                payload,
                created_at
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                "SELECT record FROM checkpoint_records
                 WHERE session_id = ?1 AND run_id = ?2
                   AND sequence_no = ?3 AND checkpoint_id = ?4",
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    sequence,
                    checkpoint_id
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        if deserialize_json_record::<AgentCheckpoint>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "checkpoint conflict for session {} run {} at sequence {sequence} checkpoint {checkpoint_id}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_exact_hitl_record<T>(
    transaction: &Transaction<'_>,
    table: &str,
    id_column: &str,
    session_id: &SessionId,
    run_id: &RunId,
    id: &str,
    payload: &str,
    expected: &T,
    updated_at: String,
    kind: &str,
) -> SessionStoreResult<()>
where
    T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord + PartialEq,
{
    let inserted = transaction
        .execute(
            &format!(
                "INSERT OR IGNORE INTO {table}
                 (session_id, run_id, {id_column}, record, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)"
            ),
            params![
                session_id.as_str(),
                run_id.as_str(),
                id,
                payload,
                updated_at
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if inserted == 0 {
        let persisted = transaction
            .query_row(
                &format!(
                    "SELECT record FROM {table}
                     WHERE session_id = ?1 AND run_id = ?2 AND {id_column} = ?3"
                ),
                params![session_id.as_str(), run_id.as_str(), id],
                |row| row.get::<_, String>(0),
            )
            .map_err(map_sqlite_session_error)?;
        if deserialize_json_record::<T>(&persisted)? != *expected {
            return Err(SessionStoreError::Failed(format!(
                "{kind} conflict for session {} run {} and id {id}",
                session_id.as_str(),
                run_id.as_str()
            )));
        }
    }
    Ok(())
}

fn replace_resolved_approval(
    transaction: &Transaction<'_>,
    approval: &ApprovalRecord,
    payload: &str,
) -> SessionStoreResult<()> {
    if approval.status == ApprovalStatus::Pending || approval.decision.is_none() {
        return Err(SessionStoreError::Failed(format!(
            "approval {} is not resolved",
            approval.approval_id
        )));
    }
    let existing_payload = transaction
        .query_row(
            "SELECT record FROM approval_records
             WHERE session_id = ?1 AND run_id = ?2 AND approval_id = ?3",
            params![
                approval.session_id.as_str(),
                approval.run_id.as_str(),
                approval.approval_id
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(approval.approval_id.clone()))?;
    let existing = deserialize_json_record::<ApprovalRecord>(&existing_payload)?;
    let same_request = existing.approval_id == approval.approval_id
        && existing.session_id == approval.session_id
        && existing.run_id == approval.run_id
        && existing.action_id == approval.action_id
        && existing.action_name == approval.action_name
        && existing.request == approval.request
        && existing.created_at == approval.created_at
        && existing.trace_context == approval.trace_context
        && existing.metadata == approval.metadata;
    if existing.status != ApprovalStatus::Pending || !same_request {
        return Err(SessionStoreError::Failed(format!(
            "approval transition conflict for {}",
            approval.approval_id
        )));
    }
    let updated = transaction
        .execute(
            "UPDATE approval_records SET record = ?1, updated_at = ?2
             WHERE session_id = ?3 AND run_id = ?4 AND approval_id = ?5",
            params![
                payload,
                approval.updated_at.to_rfc3339(),
                approval.session_id.as_str(),
                approval.run_id.as_str(),
                approval.approval_id
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Failed(format!(
            "approval transition conflict for {}",
            approval.approval_id
        )));
    }
    Ok(())
}

fn replace_resolved_deferred_tool(
    transaction: &Transaction<'_>,
    deferred: &DeferredToolRecord,
    payload: &str,
) -> SessionStoreResult<()> {
    if matches!(
        deferred.status,
        ExecutionStatus::Pending | ExecutionStatus::Running | ExecutionStatus::Waiting
    ) {
        return Err(SessionStoreError::Failed(format!(
            "deferred tool {} is not resolved",
            deferred.deferred_id
        )));
    }
    let existing_payload = transaction
        .query_row(
            "SELECT record FROM deferred_tool_records
             WHERE session_id = ?1 AND run_id = ?2 AND deferred_id = ?3",
            params![
                deferred.session_id.as_str(),
                deferred.run_id.as_str(),
                deferred.deferred_id
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(deferred.deferred_id.clone()))?;
    let existing = deserialize_json_record::<DeferredToolRecord>(&existing_payload)?;
    let same_request = existing.deferred_id == deferred.deferred_id
        && existing.session_id == deferred.session_id
        && existing.run_id == deferred.run_id
        && existing.tool_call_id == deferred.tool_call_id
        && existing.tool_name == deferred.tool_name
        && existing.request == deferred.request
        && existing.created_at == deferred.created_at
        && existing.trace_context == deferred.trace_context;
    if !matches!(
        existing.status,
        ExecutionStatus::Pending | ExecutionStatus::Waiting
    ) || !same_request
    {
        return Err(SessionStoreError::Failed(format!(
            "deferred tool transition conflict for {}",
            deferred.deferred_id
        )));
    }
    let updated = transaction
        .execute(
            "UPDATE deferred_tool_records SET record = ?1, updated_at = ?2
             WHERE session_id = ?3 AND run_id = ?4 AND deferred_id = ?5",
            params![
                payload,
                deferred.updated_at.to_rfc3339(),
                deferred.session_id.as_str(),
                deferred.run_id.as_str(),
                deferred.deferred_id
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Failed(format!(
            "deferred tool transition conflict for {}",
            deferred.deferred_id
        )));
    }
    Ok(())
}

fn load_unique_hitl_record_tx<T>(
    connection: &rusqlite::Connection,
    table: &str,
    id_column: &str,
    id: &str,
) -> SessionStoreResult<T>
where
    T: serde::de::DeserializeOwned + starweaver_core::VersionedRecord,
{
    let mut statement = connection
        .prepare(&format!(
            "SELECT record FROM {table} WHERE {id_column} = ?1"
        ))
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map(params![id], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_session_error)?;
    let records = collect_json_record_rows(rows)?;
    match records.len() {
        0 => Err(SessionStoreError::NotFound(id.to_string())),
        1 => records
            .into_iter()
            .next()
            .ok_or_else(|| SessionStoreError::NotFound(id.to_string())),
        _ => Err(SessionStoreError::Failed(format!(
            "ambiguous durable id {id}"
        ))),
    }
}

fn update_hitl_record(
    transaction: &Transaction<'_>,
    table: &str,
    id_column: &str,
    id: &str,
    payload: &str,
    updated_at: String,
) -> SessionStoreResult<()> {
    let updated = transaction
        .execute(
            &format!("UPDATE {table} SET record = ?1, updated_at = ?2 WHERE {id_column} = ?3"),
            params![payload, updated_at, id],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Failed(format!(
            "ambiguous durable id {id}"
        )));
    }
    Ok(())
}

fn load_run_ids(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
) -> SessionStoreResult<Vec<RunId>> {
    let mut statement = transaction
        .prepare("SELECT run_id FROM run_records WHERE session_id = ?1 ORDER BY sequence_no")
        .map_err(map_sqlite_session_error)?;
    let rows = statement
        .query_map(params![session_id.as_str()], |row| row.get::<_, String>(0))
        .map_err(map_sqlite_session_error)?;
    let mut run_ids = Vec::new();
    for row in rows {
        run_ids.push(RunId::from_string(row.map_err(map_sqlite_session_error)?));
    }
    Ok(run_ids)
}

fn delete_run_evidence(
    transaction: &Transaction<'_>,
    session_id: &SessionId,
    run_id: &RunId,
) -> SessionStoreResult<()> {
    for table in [
        "checkpoint_records",
        "stream_records",
        "approval_records",
        "deferred_tool_records",
        "run_context_records",
        "run_environment_records",
    ] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE session_id = ?1 AND run_id = ?2"),
                params![session_id.as_str(), run_id.as_str()],
            )
            .map_err(map_sqlite_session_error)?;
    }
    for scope in [
        format!("run:{}", run_id.as_str()),
        format!("session:{}", session_id.as_str()),
    ] {
        transaction
            .execute(
                "DELETE FROM replay_events WHERE scope = ?1",
                params![&scope],
            )
            .map_err(map_sqlite_session_error)?;
        transaction
            .execute(
                "DELETE FROM display_message_records WHERE scope = ?1",
                params![&scope],
            )
            .map_err(map_sqlite_session_error)?;
        transaction
            .execute(
                "DELETE FROM display_snapshot_records WHERE scope = ?1",
                params![scope],
            )
            .map_err(map_sqlite_session_error)?;
    }
    transaction
        .execute(
            "DELETE FROM run_records WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}
