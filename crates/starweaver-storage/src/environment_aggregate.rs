//! Atomic SQLite persistence for durable environment attachments and mounts.

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use starweaver_session::{
    AttachEnvironment, DetachEnvironment, DurableEnvironmentAttachment, DurableEnvironmentMount,
    DurableEnvironmentMountStatus, DurableEnvironmentScope, DurableEnvironmentStatus,
    ENVIRONMENT_ATTACH_OPERATION, ENVIRONMENT_DETACH_OPERATION, ENVIRONMENT_MOUNT_OPERATION,
    ENVIRONMENT_UNMOUNT_OPERATION, EnvironmentAttachmentMutationResult, EnvironmentAttachmentPage,
    EnvironmentAttachmentPageKeyProjection, EnvironmentAttachmentQuery,
    EnvironmentHostEventContext, EnvironmentMountMutationResult, EnvironmentMountQuery,
    EnvironmentMutationResult, MAX_ENVIRONMENT_MOUNTS_PER_RUN, MountEnvironmentResource,
    MutationReceipt, SessionStoreError, SessionStoreResult, UnmountEnvironmentResource,
};

use crate::{
    SqliteStorage,
    session_store::host_events::enqueue_host_event_publications_in_transaction,
    sqlite::{deserialize_json_record, map_sqlite_session_error, serialize_json_record},
};

impl SqliteStorage {
    /// Replay one exact authority-scoped attachment mutation before external provider work.
    ///
    /// # Errors
    ///
    /// Returns an idempotency conflict when the key is already bound to another fingerprint or
    /// operation, or a storage error when the durable receipt cannot be read or decoded.
    pub fn replay_environment_attachment_mutation(
        &self,
        authority_binding: &str,
        idempotency_key: &str,
        command_fingerprint: &str,
        operation: &str,
    ) -> SessionStoreResult<Option<EnvironmentAttachmentMutationResult>> {
        if authority_binding.is_empty()
            || idempotency_key.is_empty()
            || command_fingerprint.is_empty()
            || operation.is_empty()
        {
            return Err(SessionStoreError::Failed(
                "environment replay binding fields must not be empty".to_string(),
            ));
        }
        let connection = self.lock()?;
        replay_attachment(
            &connection,
            authority_binding,
            idempotency_key,
            command_fingerprint,
            operation,
        )
    }

    /// Atomically create an attachment, its authority-scoped receipt, and optional host event.
    pub fn attach_environment(
        &self,
        command: AttachEnvironment,
    ) -> SessionStoreResult<EnvironmentAttachmentMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = immediate(&mut connection)?;
        if let Some(result) = replay_attachment(
            &transaction,
            &command.context.authority_binding,
            &command.context.idempotency_key,
            &command.context.command_fingerprint,
            ENVIRONMENT_ATTACH_OPERATION,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(result);
        }
        if load_attachment(
            &transaction,
            &command.context.authority_binding,
            &command.attachment_id,
        )?
        .is_some()
        {
            return Err(SessionStoreError::AlreadyExists(format!(
                "environment attachment {}",
                command.attachment_id
            )));
        }

        let attachment = DurableEnvironmentAttachment {
            authority_binding: command.context.authority_binding.clone(),
            attachment_id: command.attachment_id,
            environment_id: command.environment_id,
            display_name: command.display_name,
            scope: command.scope,
            status: command.status,
            revision: 1,
            updated_at: command.context.occurred_at,
        };
        attachment.validate()?;
        insert_attachment(&transaction, &attachment)?;
        let result = EnvironmentAttachmentMutationResult {
            receipt: receipt(
                &command.context.authority_binding,
                &command.context.idempotency_key,
                &command.context.command_fingerprint,
                ENVIRONMENT_ATTACH_OPERATION,
                &attachment.attachment_id,
                command.context.occurred_at,
            ),
            attachment,
        };
        result.validate()?;
        insert_environment_receipt(
            &transaction,
            &command.context.authority_binding,
            ENVIRONMENT_ATTACH_OPERATION,
            &EnvironmentMutationResult::Attachment(result.clone()),
        )?;
        enqueue_optional(
            &transaction,
            command.context.host_event.as_ref(),
            &result.attachment,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Atomically revoke every non-terminal attachment owned by one connection.
    ///
    /// The caller supplies one fully projected detach command for each attachment observed under
    /// the trusted connection scope. The transaction verifies that the command set is complete
    /// before changing any state, then commits all revisions, receipts, and host events together.
    pub fn detach_connection_environments(
        &self,
        authority_binding: &str,
        connection_id: &str,
        commands: Vec<DetachEnvironment>,
    ) -> SessionStoreResult<Vec<EnvironmentAttachmentMutationResult>> {
        if authority_binding.is_empty() || connection_id.is_empty() {
            return Err(SessionStoreError::Failed(
                "environment authority and connection identities must not be empty".to_string(),
            ));
        }
        let mut command_ids = std::collections::BTreeSet::new();
        for command in &commands {
            command.validate()?;
            if command.context.authority_binding != authority_binding {
                return Err(SessionStoreError::Conflict(
                    "connection detach command authority does not match aggregate authority"
                        .to_string(),
                ));
            }
            if !command_ids.insert(command.attachment_id.clone()) {
                return Err(SessionStoreError::Conflict(format!(
                    "duplicate connection detach command for {}",
                    command.attachment_id
                )));
            }
        }

        let mut connection = self.lock()?;
        let transaction = immediate(&mut connection)?;
        let expected_scope = scope_key(&DurableEnvironmentScope::Connection {
            connection_id: connection_id.to_string(),
        })?;
        let expected_count: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM environment_attachment_records
                 WHERE authority_binding = ?1 AND scope_key = ?2 AND status != 'detached'",
                params![authority_binding, expected_scope],
                |row| row.get(0),
            )
            .map_err(map_sqlite_session_error)?;
        if usize::try_from(expected_count).ok() != Some(commands.len()) {
            return Err(SessionStoreError::Conflict(
                "connection attachment set changed during atomic revocation".to_string(),
            ));
        }

        let expected_scope = DurableEnvironmentScope::Connection {
            connection_id: connection_id.to_string(),
        };
        let mut results = Vec::with_capacity(commands.len());
        for command in commands {
            let mut attachment =
                load_attachment(&transaction, authority_binding, &command.attachment_id)?
                    .ok_or_else(|| {
                        SessionStoreError::NotFound(format!(
                            "environment attachment {}",
                            command.attachment_id
                        ))
                    })?;
            if attachment.scope != expected_scope
                || attachment.status == DurableEnvironmentStatus::Detached
            {
                return Err(SessionStoreError::Conflict(format!(
                    "environment attachment {} is not an active member of the connection aggregate",
                    attachment.attachment_id
                )));
            }
            let active_mounts: i64 = transaction
                .query_row(
                    "SELECT COUNT(*) FROM environment_mount_records
                     WHERE authority_binding = ?1 AND attachment_id = ?2 AND status = 'mounted'",
                    params![attachment.authority_binding, attachment.attachment_id],
                    |row| row.get(0),
                )
                .map_err(map_sqlite_session_error)?;
            if active_mounts != 0 {
                return Err(SessionStoreError::RunConflict(format!(
                    "connection attachment {} has active mounts",
                    attachment.attachment_id
                )));
            }
            let previous_revision = attachment.revision;
            attachment.revision = next_revision(previous_revision, "environment attachment")?;
            attachment.status = DurableEnvironmentStatus::Detached;
            attachment.updated_at = command.context.occurred_at;
            attachment.validate()?;
            update_attachment(&transaction, &attachment, previous_revision)?;
            let result = EnvironmentAttachmentMutationResult {
                receipt: receipt(
                    authority_binding,
                    &command.context.idempotency_key,
                    &command.context.command_fingerprint,
                    ENVIRONMENT_DETACH_OPERATION,
                    &attachment.attachment_id,
                    command.context.occurred_at,
                ),
                attachment,
            };
            result.validate()?;
            insert_environment_receipt(
                &transaction,
                authority_binding,
                ENVIRONMENT_DETACH_OPERATION,
                &EnvironmentMutationResult::Attachment(result.clone()),
            )?;
            enqueue_optional(
                &transaction,
                command.context.host_event.as_ref(),
                &result.attachment,
            )?;
            results.push(result);
        }
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(results)
    }

    /// Atomically terminally detach an attachment, persist its receipt, and enqueue its event.
    pub fn detach_environment(
        &self,
        command: DetachEnvironment,
    ) -> SessionStoreResult<EnvironmentAttachmentMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = immediate(&mut connection)?;
        if let Some(result) = replay_attachment(
            &transaction,
            &command.context.authority_binding,
            &command.context.idempotency_key,
            &command.context.command_fingerprint,
            ENVIRONMENT_DETACH_OPERATION,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(result);
        }
        let mut attachment = load_attachment(
            &transaction,
            &command.context.authority_binding,
            &command.attachment_id,
        )?
        .ok_or_else(|| {
            SessionStoreError::NotFound(format!("environment attachment {}", command.attachment_id))
        })?;
        if attachment.status == DurableEnvironmentStatus::Detached {
            return Err(SessionStoreError::Conflict(format!(
                "environment attachment {} is already detached",
                attachment.attachment_id
            )));
        }
        let active_mounts: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM environment_mount_records
                 WHERE authority_binding = ?1 AND attachment_id = ?2 AND status = 'mounted'",
                params![attachment.authority_binding, attachment.attachment_id],
                |row| row.get(0),
            )
            .map_err(map_sqlite_session_error)?;
        if active_mounts != 0 {
            return Err(SessionStoreError::RunConflict(format!(
                "environment attachment {} has active mounts",
                attachment.attachment_id
            )));
        }
        let previous_revision = attachment.revision;
        attachment.revision = next_revision(previous_revision, "environment attachment")?;
        attachment.status = DurableEnvironmentStatus::Detached;
        attachment.updated_at = command.context.occurred_at;
        attachment.validate()?;
        update_attachment(&transaction, &attachment, previous_revision)?;
        let result = EnvironmentAttachmentMutationResult {
            receipt: receipt(
                &command.context.authority_binding,
                &command.context.idempotency_key,
                &command.context.command_fingerprint,
                ENVIRONMENT_DETACH_OPERATION,
                &attachment.attachment_id,
                command.context.occurred_at,
            ),
            attachment,
        };
        result.validate()?;
        insert_environment_receipt(
            &transaction,
            &command.context.authority_binding,
            ENVIRONMENT_DETACH_OPERATION,
            &EnvironmentMutationResult::Attachment(result.clone()),
        )?;
        enqueue_optional(
            &transaction,
            command.context.host_event.as_ref(),
            &result.attachment,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Atomically mount a product-resolved allowlisted resource and persist only its safe label.
    pub fn mount_environment_resource(
        &self,
        command: MountEnvironmentResource,
    ) -> SessionStoreResult<EnvironmentMountMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = immediate(&mut connection)?;
        if let Some(result) = replay_mount(
            &transaction,
            &command.context.authority_binding,
            &command.context.idempotency_key,
            &command.context.command_fingerprint,
            ENVIRONMENT_MOUNT_OPERATION,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(result);
        }
        let mut attachment = load_attachment(
            &transaction,
            &command.context.authority_binding,
            &command.attachment_id,
        )?
        .ok_or_else(|| {
            SessionStoreError::NotFound(format!("environment attachment {}", command.attachment_id))
        })?;
        if !matches!(
            attachment.status,
            DurableEnvironmentStatus::Ready | DurableEnvironmentStatus::Degraded
        ) {
            return Err(SessionStoreError::Conflict(format!(
                "environment attachment {} is not mountable in status {:?}",
                attachment.attachment_id, attachment.status
            )));
        }
        if !attachment.scope.permits_run(
            command.connection_id.as_deref(),
            &command.session_id,
            &command.run_id,
        ) {
            return Err(SessionStoreError::Conflict(format!(
                "environment attachment {} scope does not permit run {}/{}",
                attachment.attachment_id, command.session_id, command.run_id
            )));
        }
        if load_mount(
            &transaction,
            &command.context.authority_binding,
            &command.mount_id,
        )?
        .is_some()
        {
            return Err(SessionStoreError::AlreadyExists(format!(
                "environment mount {}",
                command.mount_id
            )));
        }
        let active_mounts: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM environment_mount_records
                 WHERE authority_binding = ?1 AND session_id = ?2 AND run_id = ?3
                   AND status = 'mounted'",
                params![
                    command.context.authority_binding,
                    command.session_id,
                    command.run_id
                ],
                |row| row.get(0),
            )
            .map_err(map_sqlite_session_error)?;
        if active_mounts >= i64::from(MAX_ENVIRONMENT_MOUNTS_PER_RUN) {
            return Err(SessionStoreError::QuotaExceeded(format!(
                "run {}/{} already has the maximum {} active environment mounts",
                command.session_id, command.run_id, MAX_ENVIRONMENT_MOUNTS_PER_RUN
            )));
        }
        let mount = DurableEnvironmentMount {
            authority_binding: command.context.authority_binding.clone(),
            mount_id: command.mount_id,
            attachment_id: command.attachment_id,
            session_id: command.session_id,
            run_id: command.run_id,
            resource_label: command.resource_label,
            status: DurableEnvironmentMountStatus::Mounted,
            revision: 1,
            updated_at: command.context.occurred_at,
        };
        mount.validate()?;
        insert_mount(&transaction, &mount)?;
        let previous_attachment_revision = attachment.revision;
        attachment.revision =
            next_revision(previous_attachment_revision, "environment attachment")?;
        attachment.updated_at = command.context.occurred_at;
        attachment.validate()?;
        update_attachment(&transaction, &attachment, previous_attachment_revision)?;
        let result = EnvironmentMountMutationResult {
            receipt: receipt(
                &command.context.authority_binding,
                &command.context.idempotency_key,
                &command.context.command_fingerprint,
                ENVIRONMENT_MOUNT_OPERATION,
                &mount.mount_id,
                command.context.occurred_at,
            ),
            mount,
        };
        result.validate()?;
        insert_environment_receipt(
            &transaction,
            &command.context.authority_binding,
            ENVIRONMENT_MOUNT_OPERATION,
            &EnvironmentMutationResult::Mount(result.clone()),
        )?;
        enqueue_optional(
            &transaction,
            command.context.host_event.as_ref(),
            &attachment,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Atomically terminally unmount a resource, persist its receipt, and enqueue its event.
    pub fn unmount_environment_resource(
        &self,
        command: UnmountEnvironmentResource,
    ) -> SessionStoreResult<EnvironmentMountMutationResult> {
        command.validate()?;
        let mut connection = self.lock()?;
        let transaction = immediate(&mut connection)?;
        if let Some(result) = replay_mount(
            &transaction,
            &command.context.authority_binding,
            &command.context.idempotency_key,
            &command.context.command_fingerprint,
            ENVIRONMENT_UNMOUNT_OPERATION,
        )? {
            transaction.commit().map_err(map_sqlite_session_error)?;
            return Ok(result);
        }
        let mut mount = load_mount(
            &transaction,
            &command.context.authority_binding,
            &command.mount_id,
        )?
        .ok_or_else(|| {
            SessionStoreError::NotFound(format!("environment mount {}", command.mount_id))
        })?;
        if mount.status == DurableEnvironmentMountStatus::Unmounted {
            return Err(SessionStoreError::Conflict(format!(
                "environment mount {} is already unmounted",
                mount.mount_id
            )));
        }
        let mut attachment = load_attachment(
            &transaction,
            &command.context.authority_binding,
            &mount.attachment_id,
        )?
        .ok_or_else(|| {
            SessionStoreError::Conflict(format!(
                "environment mount {} references a missing attachment",
                mount.mount_id
            ))
        })?;
        let previous_revision = mount.revision;
        mount.revision = next_revision(previous_revision, "environment mount")?;
        mount.status = DurableEnvironmentMountStatus::Unmounted;
        mount.updated_at = command.context.occurred_at;
        mount.validate()?;
        update_mount(&transaction, &mount, previous_revision)?;
        let previous_attachment_revision = attachment.revision;
        attachment.revision =
            next_revision(previous_attachment_revision, "environment attachment")?;
        attachment.updated_at = command.context.occurred_at;
        attachment.validate()?;
        update_attachment(&transaction, &attachment, previous_attachment_revision)?;
        let result = EnvironmentMountMutationResult {
            receipt: receipt(
                &command.context.authority_binding,
                &command.context.idempotency_key,
                &command.context.command_fingerprint,
                ENVIRONMENT_UNMOUNT_OPERATION,
                &mount.mount_id,
                command.context.occurred_at,
            ),
            mount,
        };
        result.validate()?;
        insert_environment_receipt(
            &transaction,
            &command.context.authority_binding,
            ENVIRONMENT_UNMOUNT_OPERATION,
            &EnvironmentMutationResult::Mount(result.clone()),
        )?;
        enqueue_optional(
            &transaction,
            command.context.host_event.as_ref(),
            &attachment,
        )?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(result)
    }

    /// Load one authority-owned attachment by stable identity.
    pub fn get_environment_attachment(
        &self,
        authority_binding: &str,
        attachment_id: &str,
    ) -> SessionStoreResult<Option<DurableEnvironmentAttachment>> {
        if authority_binding.is_empty() || attachment_id.is_empty() {
            return Err(SessionStoreError::Failed(
                "environment authority and attachment identities must not be empty".to_string(),
            ));
        }
        let connection = self.lock()?;
        let payload = connection
            .query_row(
                "SELECT record FROM environment_attachment_records
                 WHERE authority_binding = ?1 AND attachment_id = ?2",
                params![authority_binding, attachment_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        let attachment = payload
            .map(|payload| deserialize_json_record::<DurableEnvironmentAttachment>(&payload))
            .transpose()?;
        if let Some(attachment) = &attachment {
            attachment.validate()?;
            if attachment.authority_binding != authority_binding
                || attachment.attachment_id != attachment_id
            {
                return Err(SessionStoreError::Conflict(format!(
                    "durable environment attachment {attachment_id} does not match its key binding"
                )));
            }
        }
        Ok(attachment)
    }

    /// Load one authority-owned mount by stable identity.
    pub fn get_environment_mount(
        &self,
        authority_binding: &str,
        mount_id: &str,
    ) -> SessionStoreResult<Option<DurableEnvironmentMount>> {
        if authority_binding.is_empty() || mount_id.is_empty() {
            return Err(SessionStoreError::Failed(
                "environment authority and mount identities must not be empty".to_string(),
            ));
        }
        let connection = self.lock()?;
        let payload = connection
            .query_row(
                "SELECT record FROM environment_mount_records
                 WHERE authority_binding = ?1 AND mount_id = ?2",
                params![authority_binding, mount_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        let mount = payload
            .map(|payload| deserialize_json_record::<DurableEnvironmentMount>(&payload))
            .transpose()?;
        if let Some(mount) = &mount {
            mount.validate()?;
            if mount.authority_binding != authority_binding || mount.mount_id != mount_id {
                return Err(SessionStoreError::Conflict(format!(
                    "durable environment mount {mount_id} does not match its key binding"
                )));
            }
        }
        Ok(mount)
    }

    /// Return all attachments owned by one trusted connection for teardown reconciliation.
    pub fn list_connection_environment_attachments(
        &self,
        authority_binding: &str,
        connection_id: &str,
    ) -> SessionStoreResult<Vec<DurableEnvironmentAttachment>> {
        if authority_binding.is_empty() || connection_id.is_empty() {
            return Err(SessionStoreError::Failed(
                "environment authority and connection identities must not be empty".to_string(),
            ));
        }
        let connection = self.lock()?;
        let expected_scope = scope_key(&DurableEnvironmentScope::Connection {
            connection_id: connection_id.to_string(),
        })?;
        let mut statement = connection
            .prepare(
                "SELECT attachment_id, updated_at, record
                 FROM environment_attachment_records
                 WHERE authority_binding = ?1 AND scope_key = ?2
                 ORDER BY updated_at DESC, attachment_id DESC",
            )
            .map_err(map_sqlite_session_error)?;
        collect_attachments(
            statement
                .query_map(params![authority_binding, expected_scope], attachment_row)
                .map_err(map_sqlite_session_error)?,
            authority_binding,
        )
    }

    /// Return a bounded stable descending-keyset page of authority-owned attachments.
    pub fn list_environment_attachments(
        &self,
        query: EnvironmentAttachmentQuery,
    ) -> SessionStoreResult<EnvironmentAttachmentPage> {
        query.validate()?;
        let connection = self.lock()?;
        let scope_key = query.scope.as_ref().map(scope_key).transpose()?;
        let fetch_limit = i64::from(query.limit) + 1;
        let cursor_time = query.after.as_ref().map(|key| storage_time(key.updated_at));
        let cursor_id = query.after.as_ref().map(|key| key.attachment_id.as_str());
        let mut records = if let Some(scope_key) = scope_key {
            let mut statement = connection
                .prepare(
                    "SELECT attachment_id, updated_at, record
                     FROM environment_attachment_records
                     WHERE authority_binding = ?1 AND scope_key = ?2
                       AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND attachment_id < ?4))
                     ORDER BY updated_at DESC, attachment_id DESC LIMIT ?5",
                )
                .map_err(map_sqlite_session_error)?;
            collect_attachments(
                statement
                    .query_map(
                        params![
                            query.authority_binding,
                            scope_key,
                            cursor_time,
                            cursor_id,
                            fetch_limit
                        ],
                        attachment_row,
                    )
                    .map_err(map_sqlite_session_error)?,
                &query.authority_binding,
            )?
        } else {
            let mut statement = connection
                .prepare(
                    "SELECT attachment_id, updated_at, record
                     FROM environment_attachment_records
                     WHERE authority_binding = ?1
                       AND (json_extract(scope_key, '$.kind') <> 'connection'
                            OR (?2 IS NOT NULL AND json_extract(scope_key, '$.connection_id') = ?2))
                       AND (?3 IS NULL OR updated_at < ?3 OR (updated_at = ?3 AND attachment_id < ?4))
                     ORDER BY updated_at DESC, attachment_id DESC LIMIT ?5",
                )
                .map_err(map_sqlite_session_error)?;
            collect_attachments(
                statement
                    .query_map(
                        params![
                            query.authority_binding,
                            query.connection_id,
                            cursor_time,
                            cursor_id,
                            fetch_limit
                        ],
                        attachment_row,
                    )
                    .map_err(map_sqlite_session_error)?,
                &query.authority_binding,
            )?
        };
        let has_more = records.len() > query.limit as usize;
        records.truncate(query.limit as usize);
        let next = if has_more {
            records
                .last()
                .map(|record| EnvironmentAttachmentPageKeyProjection {
                    updated_at: record.updated_at,
                    attachment_id: record.attachment_id.clone(),
                })
        } else {
            None
        };
        Ok(EnvironmentAttachmentPage {
            items: records,
            next,
        })
    }

    /// Return a deterministically ordered bounded list of active mounts for one run.
    pub fn list_environment_mounts(
        &self,
        query: EnvironmentMountQuery,
    ) -> SessionStoreResult<Vec<DurableEnvironmentMount>> {
        query.validate()?;
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT mount_id, record FROM environment_mount_records
                 WHERE authority_binding = ?1 AND session_id = ?2 AND run_id = ?3
                   AND status = 'mounted'
                 ORDER BY mount_id ASC LIMIT ?4",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(
                params![
                    query.authority_binding,
                    query.session_id,
                    query.run_id,
                    query.limit
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(map_sqlite_session_error)?;
        let mut mounts = Vec::new();
        for row in rows {
            let (mount_id, payload) = row.map_err(map_sqlite_session_error)?;
            let mount = deserialize_json_record::<DurableEnvironmentMount>(&payload)?;
            mount.validate()?;
            if mount.authority_binding != query.authority_binding || mount.mount_id != mount_id {
                return Err(SessionStoreError::Conflict(format!(
                    "durable environment mount {mount_id} does not match its key binding"
                )));
            }
            mounts.push(mount);
        }
        Ok(mounts)
    }
}

fn immediate(connection: &mut rusqlite::Connection) -> SessionStoreResult<Transaction<'_>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(map_sqlite_session_error)
}

fn scope_key(scope: &DurableEnvironmentScope) -> SessionStoreResult<String> {
    serde_json::to_string(scope).map_err(|error| SessionStoreError::Failed(error.to_string()))
}

fn status_key(status: DurableEnvironmentStatus) -> &'static str {
    match status {
        DurableEnvironmentStatus::Attaching => "attaching",
        DurableEnvironmentStatus::Ready => "ready",
        DurableEnvironmentStatus::Degraded => "degraded",
        DurableEnvironmentStatus::Detached => "detached",
    }
}

fn mount_status_key(status: DurableEnvironmentMountStatus) -> &'static str {
    match status {
        DurableEnvironmentMountStatus::Mounted => "mounted",
        DurableEnvironmentMountStatus::Unmounted => "unmounted",
    }
}

fn load_attachment(
    transaction: &Transaction<'_>,
    authority_binding: &str,
    attachment_id: &str,
) -> SessionStoreResult<Option<DurableEnvironmentAttachment>> {
    let payload = transaction
        .query_row(
            "SELECT record FROM environment_attachment_records
             WHERE authority_binding = ?1 AND attachment_id = ?2",
            params![authority_binding, attachment_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    let attachment = payload
        .map(|payload| deserialize_json_record::<DurableEnvironmentAttachment>(&payload))
        .transpose()?;
    if let Some(attachment) = &attachment {
        attachment.validate()?;
        if attachment.authority_binding != authority_binding
            || attachment.attachment_id != attachment_id
        {
            return Err(SessionStoreError::Conflict(format!(
                "durable environment attachment {attachment_id} does not match its key binding"
            )));
        }
    }
    Ok(attachment)
}

fn insert_attachment(
    transaction: &Transaction<'_>,
    attachment: &DurableEnvironmentAttachment,
) -> SessionStoreResult<()> {
    let revision = sqlite_revision(attachment.revision, "environment attachment")?;
    let record = serialize_json_record(attachment)?;
    transaction
        .execute(
            "INSERT INTO environment_attachment_records
             (authority_binding, attachment_id, environment_id, scope_key, status, revision,
              record, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                attachment.authority_binding,
                attachment.attachment_id,
                attachment.environment_id,
                scope_key(&attachment.scope)?,
                status_key(attachment.status),
                revision,
                record,
                storage_time(attachment.updated_at),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn update_attachment(
    transaction: &Transaction<'_>,
    attachment: &DurableEnvironmentAttachment,
    expected_revision: u64,
) -> SessionStoreResult<()> {
    let record = serialize_json_record(attachment)?;
    let updated = transaction
        .execute(
            "UPDATE environment_attachment_records
             SET status = ?3, revision = ?4, record = ?5, updated_at = ?6
             WHERE authority_binding = ?1 AND attachment_id = ?2 AND revision = ?7",
            params![
                attachment.authority_binding,
                attachment.attachment_id,
                status_key(attachment.status),
                sqlite_revision(attachment.revision, "environment attachment")?,
                record,
                storage_time(attachment.updated_at),
                sqlite_revision(expected_revision, "environment attachment")?,
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "environment attachment revision changed: {}",
            attachment.attachment_id
        )));
    }
    Ok(())
}

fn load_mount(
    transaction: &Transaction<'_>,
    authority_binding: &str,
    mount_id: &str,
) -> SessionStoreResult<Option<DurableEnvironmentMount>> {
    let payload = transaction
        .query_row(
            "SELECT record FROM environment_mount_records
             WHERE authority_binding = ?1 AND mount_id = ?2",
            params![authority_binding, mount_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    let mount = payload
        .map(|payload| deserialize_json_record::<DurableEnvironmentMount>(&payload))
        .transpose()?;
    if let Some(mount) = &mount {
        mount.validate()?;
        if mount.authority_binding != authority_binding || mount.mount_id != mount_id {
            return Err(SessionStoreError::Conflict(format!(
                "durable environment mount {mount_id} does not match its key binding"
            )));
        }
    }
    Ok(mount)
}

fn insert_mount(
    transaction: &Transaction<'_>,
    mount: &DurableEnvironmentMount,
) -> SessionStoreResult<()> {
    let record = serialize_json_record(mount)?;
    transaction
        .execute(
            "INSERT INTO environment_mount_records
             (authority_binding, mount_id, attachment_id, session_id, run_id, status, revision,
              record, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                mount.authority_binding,
                mount.mount_id,
                mount.attachment_id,
                mount.session_id,
                mount.run_id,
                mount_status_key(mount.status),
                sqlite_revision(mount.revision, "environment mount")?,
                record,
                storage_time(mount.updated_at),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn update_mount(
    transaction: &Transaction<'_>,
    mount: &DurableEnvironmentMount,
    expected_revision: u64,
) -> SessionStoreResult<()> {
    let record = serialize_json_record(mount)?;
    let updated = transaction
        .execute(
            "UPDATE environment_mount_records
             SET status = ?3, revision = ?4, record = ?5, updated_at = ?6
             WHERE authority_binding = ?1 AND mount_id = ?2 AND revision = ?7",
            params![
                mount.authority_binding,
                mount.mount_id,
                mount_status_key(mount.status),
                sqlite_revision(mount.revision, "environment mount")?,
                record,
                storage_time(mount.updated_at),
                sqlite_revision(expected_revision, "environment mount")?,
            ],
        )
        .map_err(map_sqlite_session_error)?;
    if updated != 1 {
        return Err(SessionStoreError::Conflict(format!(
            "environment mount revision changed: {}",
            mount.mount_id
        )));
    }
    Ok(())
}

fn load_environment_receipt(
    connection: &rusqlite::Connection,
    authority_binding: &str,
    idempotency_key: &str,
) -> SessionStoreResult<Option<(String, String, String)>> {
    connection
        .query_row(
            "SELECT command_fingerprint, operation, record
             FROM environment_mutation_receipts
             WHERE authority_binding = ?1 AND idempotency_key = ?2",
            params![authority_binding, idempotency_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite_session_error)
}

fn replay_attachment(
    connection: &rusqlite::Connection,
    authority_binding: &str,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
) -> SessionStoreResult<Option<EnvironmentAttachmentMutationResult>> {
    let Some((persisted_fingerprint, persisted_operation, payload)) =
        load_environment_receipt(connection, authority_binding, idempotency_key)?
    else {
        return Ok(None);
    };
    validate_receipt_binding(
        idempotency_key,
        fingerprint,
        operation,
        &persisted_fingerprint,
        &persisted_operation,
    )?;
    let EnvironmentMutationResult::Attachment(result) =
        deserialize_json_record::<EnvironmentMutationResult>(&payload)?
    else {
        return Err(SessionStoreError::Conflict(
            "environment receipt payload kind does not match operation".to_string(),
        ));
    };
    validate_persisted_attachment_result(
        &result,
        authority_binding,
        idempotency_key,
        fingerprint,
        operation,
    )?;
    Ok(Some(result.replayed_projection()))
}

fn replay_mount(
    connection: &rusqlite::Connection,
    authority_binding: &str,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
) -> SessionStoreResult<Option<EnvironmentMountMutationResult>> {
    let Some((persisted_fingerprint, persisted_operation, payload)) =
        load_environment_receipt(connection, authority_binding, idempotency_key)?
    else {
        return Ok(None);
    };
    validate_receipt_binding(
        idempotency_key,
        fingerprint,
        operation,
        &persisted_fingerprint,
        &persisted_operation,
    )?;
    let EnvironmentMutationResult::Mount(result) =
        deserialize_json_record::<EnvironmentMutationResult>(&payload)?
    else {
        return Err(SessionStoreError::Conflict(
            "environment receipt payload kind does not match operation".to_string(),
        ));
    };
    validate_persisted_mount_result(
        &result,
        authority_binding,
        idempotency_key,
        fingerprint,
        operation,
    )?;
    Ok(Some(result.replayed_projection()))
}

fn validate_receipt_binding(
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
    persisted_fingerprint: &str,
    persisted_operation: &str,
) -> SessionStoreResult<()> {
    if persisted_fingerprint != fingerprint {
        return Err(SessionStoreError::IdempotencyConflict(format!(
            "environment key {idempotency_key} is already bound to another fingerprint"
        )));
    }
    if persisted_operation != operation {
        return Err(SessionStoreError::IdempotencyConflict(format!(
            "environment key {idempotency_key} is already bound to operation {persisted_operation}"
        )));
    }
    Ok(())
}

fn validate_persisted_attachment_result(
    result: &EnvironmentAttachmentMutationResult,
    authority_binding: &str,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
) -> SessionStoreResult<()> {
    result.validate()?;
    validate_persisted_receipt(&result.receipt, idempotency_key, fingerprint, operation)?;
    if result.attachment.authority_binding != authority_binding {
        return Err(SessionStoreError::Conflict(
            "environment attachment receipt authority mismatch".to_string(),
        ));
    }
    Ok(())
}

fn validate_persisted_mount_result(
    result: &EnvironmentMountMutationResult,
    authority_binding: &str,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
) -> SessionStoreResult<()> {
    result.validate()?;
    validate_persisted_receipt(&result.receipt, idempotency_key, fingerprint, operation)?;
    if result.mount.authority_binding != authority_binding {
        return Err(SessionStoreError::Conflict(
            "environment mount receipt authority mismatch".to_string(),
        ));
    }
    Ok(())
}

fn validate_persisted_receipt(
    receipt: &MutationReceipt,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
) -> SessionStoreResult<()> {
    if receipt.idempotency_key != idempotency_key
        || receipt.fingerprint != fingerprint
        || receipt.operation != operation
        || receipt.replayed
    {
        return Err(SessionStoreError::Conflict(format!(
            "durable environment receipt {} does not match its key binding",
            receipt.receipt_id
        )));
    }
    Ok(())
}

fn insert_environment_receipt(
    transaction: &Transaction<'_>,
    authority_binding: &str,
    operation: &str,
    result: &EnvironmentMutationResult,
) -> SessionStoreResult<()> {
    let receipt = match result {
        EnvironmentMutationResult::Attachment(result) => &result.receipt,
        EnvironmentMutationResult::Mount(result) => &result.receipt,
    };
    let record = serialize_json_record(result)?;
    transaction
        .execute(
            "INSERT INTO environment_mutation_receipts
             (authority_binding, idempotency_key, command_fingerprint, operation, target_ref,
              receipt_id, record, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                authority_binding,
                receipt.idempotency_key,
                receipt.fingerprint,
                operation,
                receipt.target_ref,
                receipt.receipt_id,
                record,
                storage_time(receipt.created_at),
            ],
        )
        .map_err(map_sqlite_session_error)?;
    Ok(())
}

fn receipt(
    authority_binding: &str,
    idempotency_key: &str,
    fingerprint: &str,
    operation: &str,
    target_ref: &str,
    occurred_at: chrono::DateTime<chrono::Utc>,
) -> MutationReceipt {
    MutationReceipt {
        receipt_id: mutation_receipt_id(authority_binding, idempotency_key),
        idempotency_key: idempotency_key.to_string(),
        fingerprint: fingerprint.to_string(),
        operation: operation.to_string(),
        state: "applied".to_string(),
        target_ref: target_ref.to_string(),
        reconciliation_required: false,
        replayed: false,
        created_at: occurred_at,
    }
}

fn mutation_receipt_id(authority_binding: &str, idempotency_key: &str) -> String {
    let mut digest = Sha256::new();
    for component in ["environment.mutation", authority_binding, idempotency_key] {
        digest.update(component.len().to_string().as_bytes());
        digest.update(b":");
        digest.update(component.as_bytes());
        digest.update(b";");
    }
    format!("mutation-receipt-sha256:{:x}", digest.finalize())
}

fn enqueue_optional(
    transaction: &Transaction<'_>,
    host_event: Option<&EnvironmentHostEventContext>,
    attachment: &DurableEnvironmentAttachment,
) -> SessionStoreResult<()> {
    if let Some(host_event) = host_event {
        let publication = host_event.publication(attachment)?;
        enqueue_host_event_publications_in_transaction(transaction, &[publication])?;
    }
    Ok(())
}

fn next_revision(revision: u64, label: &str) -> SessionStoreResult<u64> {
    revision
        .checked_add(1)
        .ok_or_else(|| SessionStoreError::Conflict(format!("{label} revision is exhausted")))
}

fn sqlite_revision(revision: u64, label: &str) -> SessionStoreResult<i64> {
    i64::try_from(revision)
        .map_err(|_| SessionStoreError::Conflict(format!("{label} revision exceeds SQLite range")))
}

fn storage_time(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
}

fn attachment_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<(String, String, String)> {
    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
}

fn collect_attachments(
    rows: rusqlite::MappedRows<
        '_,
        impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<(String, String, String)>,
    >,
    authority_binding: &str,
) -> SessionStoreResult<Vec<DurableEnvironmentAttachment>> {
    let mut records = Vec::new();
    for row in rows {
        let (attachment_id, updated_at, payload) = row.map_err(map_sqlite_session_error)?;
        let attachment = deserialize_json_record::<DurableEnvironmentAttachment>(&payload)?;
        attachment.validate()?;
        if attachment.authority_binding != authority_binding
            || attachment.attachment_id != attachment_id
            || storage_time(attachment.updated_at) != updated_at
        {
            return Err(SessionStoreError::Conflict(format!(
                "durable environment attachment {attachment_id} does not match its indexed projection"
            )));
        }
        records.push(attachment);
    }
    Ok(records)
}
