//! Atomic SQLite domain operations for durable human interactions.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use starweaver_core::VersionedRecord;
use starweaver_session::{
    APPROVAL_DECIDE_OPERATION, ASK_USER_QUESTION_ACTION, ApprovalDecision, ApprovalMutationResult,
    ApprovalRecord, ApprovalStatus, CLARIFICATION_ANSWERS_METADATA_KEY,
    CLARIFICATION_RESOLVE_OPERATION, CLARIFICATION_RESPONSE_METADATA_KEY,
    ClarificationMutationResult, ClarificationResolution, DEFERRED_COMPLETE_OPERATION,
    DEFERRED_FAIL_OPERATION, DecideApproval, DeferredMutationResult, DeferredToolRecord,
    ExecutionStatus, InteractionMutationContext, MutationReceipt, ResolveClarification,
    ResolveDeferredTool, SessionStoreError, SessionStoreResult, validate_clarification_answers,
};

use crate::{
    SqliteStorage,
    session_store::host_events::enqueue_host_event_publications_in_transaction,
    sqlite::{deserialize_json_record, map_sqlite_session_error, serialize_json_record},
};

impl SqliteStorage {
    /// Atomically decide a pending approval, persist a durable receipt, and optionally enqueue a
    /// product-neutral host event in the same SQLite `IMMEDIATE` transaction.
    ///
    /// Exact authority-scoped idempotent replay returns the original result with
    /// `receipt.replayed = true` and performs no state, revision, receipt, or event write.
    pub fn decide_approval_atomic(
        &self,
        command: DecideApproval,
    ) -> SessionStoreResult<ApprovalMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let target_ref = interaction_target(
            "approval",
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.approval_id,
        );
        if let Some(replay) = load_exact_replay::<ApprovalMutationResult>(
            &transaction,
            &command.context,
            APPROVAL_DECIDE_OPERATION,
            &target_ref,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(replay.replayed_projection());
        }

        let mut approval = load_approval(
            &transaction,
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.approval_id,
        )?;
        require_revision(
            "approval",
            &command.approval_id,
            approval.revision,
            command.context.expected_revision,
        )?;
        if approval.status != ApprovalStatus::Pending || approval.decision.is_some() {
            return Err(SessionStoreError::Conflict(format!(
                "approval {} is not pending",
                command.approval_id
            )));
        }
        approval.revision = next_revision("approval", &command.approval_id, approval.revision)?;
        approval.status = command.decision.status;
        approval.updated_at = command.context.occurred_at;
        approval.decision = Some(command.decision);

        let receipt = mutation_receipt(
            &command.context,
            APPROVAL_DECIDE_OPERATION,
            approval_status_name(approval.status),
            &target_ref,
        );
        let result = ApprovalMutationResult { approval, receipt };
        update_approval(&transaction, &result.approval)?;
        insert_result_receipt(
            &transaction,
            &command.context,
            APPROVAL_DECIDE_OPERATION,
            &target_ref,
            &result.receipt,
            &result,
        )?;
        enqueue_optional_event(&transaction, command.context.host_event_publication)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Atomically complete or fail a waiting deferred tool with revision, receipt, and optional
    /// host-event publication committed together.
    pub fn resolve_deferred_tool_atomic(
        &self,
        command: ResolveDeferredTool,
    ) -> SessionStoreResult<DeferredMutationResult> {
        command.validate()?;
        let operation = command.outcome.operation();
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let target_ref = interaction_target(
            "deferred",
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.deferred_id,
        );
        if let Some(replay) = load_exact_replay::<DeferredMutationResult>(
            &transaction,
            &command.context,
            operation,
            &target_ref,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(replay.replayed_projection());
        }

        let mut deferred = load_deferred(
            &transaction,
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.deferred_id,
        )?;
        require_revision(
            "deferred tool",
            &command.deferred_id,
            deferred.revision,
            command.context.expected_revision,
        )?;
        if !matches!(
            deferred.status,
            ExecutionStatus::Pending | ExecutionStatus::Waiting
        ) {
            return Err(SessionStoreError::Conflict(format!(
                "deferred tool {} is already terminal",
                command.deferred_id
            )));
        }
        deferred.revision =
            next_revision("deferred tool", &command.deferred_id, deferred.revision)?;
        deferred.status = command.outcome.status();
        let (response, metadata) = command.outcome.parts();
        deferred.response = response.clone();
        deferred.metadata.extend(metadata.clone());
        deferred.updated_at = command.context.occurred_at;

        let receipt = mutation_receipt(
            &command.context,
            operation,
            execution_status_name(deferred.status),
            &target_ref,
        );
        let result = DeferredMutationResult { deferred, receipt };
        update_deferred(&transaction, &result.deferred)?;
        insert_result_receipt(
            &transaction,
            &command.context,
            operation,
            &target_ref,
            &result.receipt,
            &result,
        )?;
        enqueue_optional_event(&transaction, command.context.host_event_publication)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Atomically resolve a clarification backed by a pending `ask_user_question` approval.
    ///
    /// Durable questions/options and every supplied answer are validated before any authoritative
    /// update. A non-clarification approval, malformed request, mismatched question, unknown or
    /// duplicate option, illegal multi-selection, or empty answer fails the transaction without
    /// a revision, receipt, or event write.
    pub fn resolve_clarification_atomic(
        &self,
        command: ResolveClarification,
    ) -> SessionStoreResult<ClarificationMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let target_ref = interaction_target(
            "clarification",
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.clarification_id,
        );
        if let Some(replay) = load_exact_replay::<ClarificationMutationResult>(
            &transaction,
            &command.context,
            CLARIFICATION_RESOLVE_OPERATION,
            &target_ref,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(replay.replayed_projection());
        }

        let mut approval = load_approval(
            &transaction,
            command.session_id.as_str(),
            command.run_id.as_str(),
            &command.clarification_id,
        )?;
        require_revision(
            "clarification",
            &command.clarification_id,
            approval.revision,
            command.context.expected_revision,
        )?;
        if approval.action_name != ASK_USER_QUESTION_ACTION {
            return Err(SessionStoreError::Conflict(format!(
                "approval {} is not an ask_user_question clarification",
                command.clarification_id
            )));
        }
        if approval.status != ApprovalStatus::Pending || approval.decision.is_some() {
            return Err(SessionStoreError::Conflict(format!(
                "clarification {} is not pending",
                command.clarification_id
            )));
        }
        let (questions, answers) =
            validate_clarification_answers(&approval.request, &command.answers)?;
        let answers_value = serde_json::to_value(&answers).map_err(|error| {
            SessionStoreError::Failed(format!("failed to encode clarification answers: {error}"))
        })?;
        let mut decision_metadata = starweaver_core::Metadata::default();
        decision_metadata.insert(
            CLARIFICATION_ANSWERS_METADATA_KEY.to_string(),
            answers_value,
        );
        if let Some(response) = &command.response {
            decision_metadata.insert(
                CLARIFICATION_RESPONSE_METADATA_KEY.to_string(),
                serde_json::Value::String(response.clone()),
            );
        }
        approval.revision = next_revision(
            "clarification",
            &command.clarification_id,
            approval.revision,
        )?;
        approval.status = ApprovalStatus::Approved;
        approval.updated_at = command.context.occurred_at;
        approval.decision = Some(ApprovalDecision {
            status: ApprovalStatus::Approved,
            decided_by: command.resolved_by,
            decided_at: command.context.occurred_at,
            reason: None,
            metadata: decision_metadata,
        });

        let clarification = ClarificationResolution {
            clarification_id: command.clarification_id,
            session_id: command.session_id,
            run_id: command.run_id,
            questions,
            answers,
            response: command.response,
            revision: approval.revision,
            resolved_at: command.context.occurred_at,
        };
        let receipt = mutation_receipt(
            &command.context,
            CLARIFICATION_RESOLVE_OPERATION,
            "resolved",
            &target_ref,
        );
        let result = ClarificationMutationResult {
            clarification,
            approval,
            receipt,
        };
        update_approval(&transaction, &result.approval)?;
        insert_result_receipt(
            &transaction,
            &command.context,
            CLARIFICATION_RESOLVE_OPERATION,
            &target_ref,
            &result.receipt,
            &result,
        )?;
        enqueue_optional_event(&transaction, command.context.host_event_publication)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }
}

fn load_approval(
    transaction: &Transaction<'_>,
    session_id: &str,
    run_id: &str,
    approval_id: &str,
) -> SessionStoreResult<ApprovalRecord> {
    let payload = transaction
        .query_row(
            "SELECT record FROM approval_records
             WHERE session_id = ?1 AND run_id = ?2 AND approval_id = ?3",
            params![session_id, run_id, approval_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(format!("approval {approval_id}")))?;
    let record = deserialize_json_record::<ApprovalRecord>(&payload)?;
    if record.session_id.as_str() != session_id
        || record.run_id.as_str() != run_id
        || record.approval_id != approval_id
    {
        return Err(SessionStoreError::Conflict(format!(
            "durable approval identity mismatch for {approval_id}"
        )));
    }
    if record.revision == 0 {
        return Err(SessionStoreError::Conflict(format!(
            "durable approval {approval_id} has revision zero"
        )));
    }
    Ok(record)
}

fn load_deferred(
    transaction: &Transaction<'_>,
    session_id: &str,
    run_id: &str,
    deferred_id: &str,
) -> SessionStoreResult<DeferredToolRecord> {
    let payload = transaction
        .query_row(
            "SELECT record FROM deferred_tool_records
             WHERE session_id = ?1 AND run_id = ?2 AND deferred_id = ?3",
            params![session_id, run_id, deferred_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?
        .ok_or_else(|| SessionStoreError::NotFound(format!("deferred tool {deferred_id}")))?;
    let record = deserialize_json_record::<DeferredToolRecord>(&payload)?;
    if record.session_id.as_str() != session_id
        || record.run_id.as_str() != run_id
        || record.deferred_id != deferred_id
    {
        return Err(SessionStoreError::Conflict(format!(
            "durable deferred tool identity mismatch for {deferred_id}"
        )));
    }
    if record.revision == 0 {
        return Err(SessionStoreError::Conflict(format!(
            "durable deferred tool {deferred_id} has revision zero"
        )));
    }
    Ok(record)
}

fn update_approval(
    transaction: &Transaction<'_>,
    approval: &ApprovalRecord,
) -> SessionStoreResult<()> {
    let payload = serialize_json_record(approval)?;
    let changed = transaction
        .execute(
            "UPDATE approval_records SET record = ?4, updated_at = ?5
             WHERE session_id = ?1 AND run_id = ?2 AND approval_id = ?3",
            params![
                approval.session_id.as_str(),
                approval.run_id.as_str(),
                approval.approval_id,
                payload,
                approval.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if changed != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "approval {} disappeared during mutation",
            approval.approval_id
        )));
    }
    Ok(())
}

fn update_deferred(
    transaction: &Transaction<'_>,
    deferred: &DeferredToolRecord,
) -> SessionStoreResult<()> {
    let payload = serialize_json_record(deferred)?;
    let changed = transaction
        .execute(
            "UPDATE deferred_tool_records SET record = ?4, updated_at = ?5
             WHERE session_id = ?1 AND run_id = ?2 AND deferred_id = ?3",
            params![
                deferred.session_id.as_str(),
                deferred.run_id.as_str(),
                deferred.deferred_id,
                payload,
                deferred.updated_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if changed != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "deferred tool {} disappeared during mutation",
            deferred.deferred_id
        )));
    }
    Ok(())
}

trait InteractionMutationResultEvidence {
    fn receipt(&self) -> &MutationReceipt;
}

impl InteractionMutationResultEvidence for ApprovalMutationResult {
    fn receipt(&self) -> &MutationReceipt {
        &self.receipt
    }
}

impl InteractionMutationResultEvidence for DeferredMutationResult {
    fn receipt(&self) -> &MutationReceipt {
        &self.receipt
    }
}

impl InteractionMutationResultEvidence for ClarificationMutationResult {
    fn receipt(&self) -> &MutationReceipt {
        &self.receipt
    }
}

fn load_exact_replay<T>(
    transaction: &Transaction<'_>,
    context: &InteractionMutationContext,
    operation: &str,
    target_ref: &str,
) -> SessionStoreResult<Option<T>>
where
    T: DeserializeOwned + InteractionMutationResultEvidence + VersionedRecord,
{
    let row = transaction
        .query_row(
            "SELECT command_fingerprint, operation, target_ref, record
             FROM interaction_mutation_receipts
             WHERE authority_binding = ?1 AND idempotency_key = ?2",
            params![context.authority_binding, context.idempotency_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    let Some((fingerprint, persisted_operation, persisted_target, payload)) = row else {
        return Ok(None);
    };
    if fingerprint != context.command_fingerprint {
        return Err(SessionStoreError::IdempotencyConflict(format!(
            "interaction key {} is already bound to another fingerprint",
            context.idempotency_key
        )));
    }
    if persisted_operation != operation || persisted_target != target_ref {
        return Err(SessionStoreError::IdempotencyConflict(format!(
            "interaction key {} is already bound to another operation or target",
            context.idempotency_key
        )));
    }
    let result = deserialize_json_record::<T>(&payload)?;
    let receipt = result.receipt();
    receipt.validate()?;
    if receipt.replayed
        || receipt.idempotency_key != context.idempotency_key
        || receipt.fingerprint != fingerprint
        || receipt.operation != operation
        || receipt.target_ref != target_ref
    {
        return Err(SessionStoreError::Conflict(format!(
            "durable interaction receipt {} does not match its key binding",
            receipt.receipt_id
        )));
    }
    Ok(Some(result))
}

fn insert_result_receipt<T>(
    transaction: &Transaction<'_>,
    context: &InteractionMutationContext,
    operation: &str,
    target_ref: &str,
    receipt: &MutationReceipt,
    result: &T,
) -> SessionStoreResult<()>
where
    T: serde::Serialize + VersionedRecord,
{
    receipt.validate()?;
    if receipt.replayed
        || receipt.operation != operation
        || receipt.target_ref != target_ref
        || receipt.idempotency_key != context.idempotency_key
        || receipt.fingerprint != context.command_fingerprint
    {
        return Err(SessionStoreError::Conflict(
            "interaction mutation receipt does not match its command binding".to_string(),
        ));
    }
    transaction
        .execute(
            "INSERT INTO interaction_mutation_receipts
             (authority_binding, idempotency_key, command_fingerprint, operation, target_ref,
              receipt_id, record, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                context.authority_binding,
                context.idempotency_key,
                context.command_fingerprint,
                operation,
                target_ref,
                receipt.receipt_id,
                serialize_json_record(result)?,
                receipt.created_at.to_rfc3339(),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn mutation_receipt(
    context: &InteractionMutationContext,
    operation: &str,
    state: &str,
    target_ref: &str,
) -> MutationReceipt {
    MutationReceipt {
        receipt_id: mutation_receipt_id(
            operation,
            &context.authority_binding,
            &context.idempotency_key,
        ),
        idempotency_key: context.idempotency_key.clone(),
        fingerprint: context.command_fingerprint.clone(),
        operation: operation.to_string(),
        state: state.to_string(),
        target_ref: target_ref.to_string(),
        reconciliation_required: false,
        replayed: false,
        created_at: context.occurred_at,
    }
}

fn mutation_receipt_id(operation: &str, authority: &str, key: &str) -> String {
    let mut digest = Sha256::new();
    for component in [operation, authority, key] {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    format!("mutation-receipt-sha256:{:x}", digest.finalize())
}

fn interaction_target(kind: &str, session_id: &str, run_id: &str, id: &str) -> String {
    let mut digest = Sha256::new();
    for component in [kind, session_id, run_id, id] {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    format!("{kind}-sha256:{:x}", digest.finalize())
}

fn require_revision(kind: &str, id: &str, actual: u64, expected: u64) -> SessionStoreResult<()> {
    if actual != expected {
        return Err(SessionStoreError::Conflict(format!(
            "{kind} {id} revision mismatch: expected {expected}, actual {actual}"
        )));
    }
    Ok(())
}

fn next_revision(kind: &str, id: &str, revision: u64) -> SessionStoreResult<u64> {
    revision
        .checked_add(1)
        .ok_or_else(|| SessionStoreError::Conflict(format!("{kind} {id} revision is exhausted")))
}

fn enqueue_optional_event(
    transaction: &Transaction<'_>,
    publication: Option<starweaver_session::PendingHostEventPublication>,
) -> SessionStoreResult<()> {
    if let Some(publication) = publication {
        enqueue_host_event_publications_in_transaction(transaction, &[publication])?;
    }
    Ok(())
}

const fn approval_status_name(status: ApprovalStatus) -> &'static str {
    match status {
        ApprovalStatus::Pending => "pending",
        ApprovalStatus::Approved => "approved",
        ApprovalStatus::Denied => "denied",
        ApprovalStatus::Expired => "expired",
        ApprovalStatus::Cancelled => "cancelled",
    }
}

const fn execution_status_name(status: ExecutionStatus) -> &'static str {
    match status {
        ExecutionStatus::Pending => "pending",
        ExecutionStatus::Running => "running",
        ExecutionStatus::Waiting => "waiting",
        ExecutionStatus::Completed => "completed",
        ExecutionStatus::Failed => "failed",
        ExecutionStatus::Cancelled => "cancelled",
    }
}

const _: [&str; 2] = [DEFERRED_COMPLETE_OPERATION, DEFERRED_FAIL_OPERATION];
