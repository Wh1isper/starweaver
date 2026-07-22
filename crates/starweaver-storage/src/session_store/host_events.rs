use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params, params_from_iter};
use starweaver_session::{
    DurableHostEventClass, DurableHostEventPage, DurableHostEventQuery, DurableHostEventRecord,
    DurableHostEventScope, EventPublicationKey, MAX_HOST_EVENT_PAGE_SIZE,
    PendingHostEventPublication, SessionStoreError, SessionStoreResult,
};

use crate::sqlite::{
    collect_json_record_rows, deserialize_json_record, map_display_session_error,
    map_sqlite_session_error, serialize_json_record,
};

use super::SqliteSessionStore;

fn validate_batch_limit(limit: usize) -> SessionStoreResult<i64> {
    if !(1..=MAX_HOST_EVENT_PAGE_SIZE).contains(&limit) {
        return Err(SessionStoreError::Failed(format!(
            "host event batch limit must be between 1 and {MAX_HOST_EVENT_PAGE_SIZE}"
        )));
    }
    i64::try_from(limit).map_err(map_display_session_error)
}

fn publication_conflict(publication_key: &EventPublicationKey) -> SessionStoreError {
    SessionStoreError::Conflict(format!(
        "host event publication conflict for {}",
        publication_key.as_str()
    ))
}

fn scope_columns(scope: &DurableHostEventScope) -> (&'static str, Option<&str>, Option<&str>) {
    (
        scope.kind(),
        scope.session_id().map(starweaver_core::SessionId::as_str),
        scope.run_id().map(starweaver_core::RunId::as_str),
    )
}

fn load_materialized_by_publication_key(
    transaction: &Transaction<'_>,
    publication_key: &EventPublicationKey,
) -> SessionStoreResult<Option<DurableHostEventRecord>> {
    let payload = transaction
        .query_row(
            "SELECT record FROM host_event_records WHERE publication_key = ?1",
            params![publication_key.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(map_sqlite_session_error)?;
    payload
        .map(|payload| deserialize_json_record(&payload))
        .transpose()
}

fn ensure_event_id_available(
    transaction: &Transaction<'_>,
    publication: &PendingHostEventPublication,
) -> SessionStoreResult<()> {
    for table in ["host_event_records", "host_event_publication_outbox"] {
        let sql = format!("SELECT publication_key FROM {table} WHERE event_id = ?1");
        let existing = transaction
            .query_row(&sql, params![publication.event_id], |row| {
                row.get::<_, String>(0)
            })
            .optional()
            .map_err(map_sqlite_session_error)?;
        if existing
            .as_deref()
            .is_some_and(|key| key != publication.publication_key.as_str())
        {
            return Err(SessionStoreError::Conflict(format!(
                "host event identity conflict for {}",
                publication.event_id
            )));
        }
    }
    Ok(())
}

pub fn enqueue_host_event_publications_in_transaction(
    transaction: &Transaction<'_>,
    publications: &[PendingHostEventPublication],
) -> SessionStoreResult<()> {
    let mut batch_by_key = BTreeMap::new();
    let mut batch_event_ids = BTreeMap::new();
    for publication in publications {
        publication.validate()?;
        if let Some(existing) =
            batch_by_key.insert(publication.publication_key.clone(), publication.clone())
            && existing != *publication
        {
            return Err(publication_conflict(&publication.publication_key));
        }
        if let Some(existing_key) = batch_event_ids.insert(
            publication.event_id.clone(),
            publication.publication_key.clone(),
        ) && existing_key != publication.publication_key
        {
            return Err(SessionStoreError::Conflict(format!(
                "host event identity conflict for {}",
                publication.event_id
            )));
        }
    }

    let mut last_sequence = transaction
        .query_row(
            "SELECT last_sequence FROM host_event_outbox_state WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(map_sqlite_session_error)?;
    let mut inserted = false;
    let mut visited = BTreeSet::new();
    for publication in publications {
        if !visited.insert(publication.publication_key.clone()) {
            continue;
        }
        if let Some(existing) =
            load_materialized_by_publication_key(transaction, &publication.publication_key)?
        {
            if existing.pending_projection() != *publication {
                return Err(publication_conflict(&publication.publication_key));
            }
            continue;
        }
        let existing_outbox = transaction
            .query_row(
                "SELECT record FROM host_event_publication_outbox WHERE publication_key = ?1",
                params![publication.publication_key.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite_session_error)?;
        if let Some(payload) = existing_outbox {
            let existing = deserialize_json_record::<PendingHostEventPublication>(&payload)?;
            if existing != *publication {
                return Err(publication_conflict(&publication.publication_key));
            }
            continue;
        }
        ensure_event_id_available(transaction, publication)?;
        last_sequence = last_sequence.checked_add(1).ok_or_else(|| {
            SessionStoreError::Failed("host event outbox sequence space is exhausted".to_string())
        })?;
        let payload = serialize_json_record(publication)?;
        let (scope_kind, session_id, run_id) = scope_columns(&publication.scope);
        transaction
            .execute(
                "INSERT INTO host_event_publication_outbox
                 (publication_key, enqueue_sequence, event_id, scope_kind, session_id, run_id,
                  event_class, record, occurred_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                params![
                    publication.publication_key.as_str(),
                    last_sequence,
                    publication.event_id,
                    scope_kind,
                    session_id,
                    run_id,
                    publication.event_class.as_str(),
                    payload,
                    publication.occurred_at.to_rfc3339(),
                ],
            )
            .map_err(map_sqlite_session_error)?;
        inserted = true;
    }
    if inserted {
        transaction
            .execute(
                "UPDATE host_event_outbox_state SET last_sequence = ?1 WHERE singleton = 1",
                params![last_sequence],
            )
            .map_err(map_sqlite_session_error)?;
    }
    Ok(())
}

fn scope_predicate(
    scope: &DurableHostEventScope,
    values: &mut Vec<rusqlite::types::Value>,
) -> &'static str {
    match scope {
        DurableHostEventScope::Global => "1 = 1",
        DurableHostEventScope::Session { session_id } => {
            values.push(session_id.as_str().to_string().into());
            "session_id = ?"
        }
        DurableHostEventScope::Run { session_id, run_id } => {
            values.push(session_id.as_str().to_string().into());
            values.push(run_id.as_str().to_string().into());
            "scope_kind = 'run' AND session_id = ? AND run_id = ?"
        }
    }
}

fn filtered_event_sql(
    prefix: &str,
    scope: &DurableHostEventScope,
    classes: &BTreeSet<DurableHostEventClass>,
    after_position: Option<u64>,
    limit: Option<usize>,
) -> SessionStoreResult<(String, Vec<rusqlite::types::Value>)> {
    if classes.is_empty() {
        return Err(SessionStoreError::Failed(
            "durable host event selection requires at least one event class".to_string(),
        ));
    }
    let mut values = Vec::new();
    let scope_sql = scope_predicate(scope, &mut values);
    let after = after_position.unwrap_or(0);
    values.push(
        i64::try_from(after)
            .map_err(map_display_session_error)?
            .into(),
    );
    let class_placeholders = classes
        .iter()
        .map(|class| {
            values.push(class.as_str().to_string().into());
            "?"
        })
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!(
        "{prefix} FROM host_event_records
         WHERE {scope_sql} AND position > ? AND event_class IN ({class_placeholders})"
    );
    if let Some(limit) = limit {
        values.push(
            i64::try_from(limit)
                .map_err(map_display_session_error)?
                .into(),
        );
        sql.push_str(" ORDER BY position ASC LIMIT ?");
    }
    Ok((sql, values))
}

impl SqliteSessionStore {
    pub(super) fn enqueue_host_event_publications_sync(
        &self,
        publications: Vec<PendingHostEventPublication>,
    ) -> SessionStoreResult<()> {
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        enqueue_host_event_publications_in_transaction(&transaction, &publications)?;
        transaction.commit().map_err(map_sqlite_session_error)
    }

    pub(super) fn pending_host_event_publications_sync(
        &self,
        limit: usize,
    ) -> SessionStoreResult<Vec<PendingHostEventPublication>> {
        let limit = validate_batch_limit(limit)?;
        let connection = self.lock()?;
        let mut statement = connection
            .prepare(
                "SELECT record FROM host_event_publication_outbox
                 ORDER BY enqueue_sequence ASC LIMIT ?1",
            )
            .map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(params![limit], |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        collect_json_record_rows(rows)
    }

    pub(super) fn materialize_host_event_publications_sync(
        &self,
        limit: usize,
    ) -> SessionStoreResult<Vec<DurableHostEventRecord>> {
        let limit = validate_batch_limit(limit)?;
        let mut connection = self.lock()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite_session_error)?;
        let pending = {
            let mut statement = transaction
                .prepare(
                    "SELECT record FROM host_event_publication_outbox
                     ORDER BY enqueue_sequence ASC LIMIT ?1",
                )
                .map_err(map_sqlite_session_error)?;
            let rows = statement
                .query_map(params![limit], |row| row.get::<_, String>(0))
                .map_err(map_sqlite_session_error)?;
            collect_json_record_rows::<PendingHostEventPublication>(rows)?
        };
        let mut last_position = transaction
            .query_row(
                "SELECT last_position FROM host_event_log_state WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .map_err(map_sqlite_session_error)?;
        let mut materialized = Vec::with_capacity(pending.len());
        for publication in pending {
            if let Some(existing) =
                load_materialized_by_publication_key(&transaction, &publication.publication_key)?
            {
                if existing.pending_projection() != publication {
                    return Err(publication_conflict(&publication.publication_key));
                }
                transaction
                    .execute(
                        "DELETE FROM host_event_publication_outbox WHERE publication_key = ?1",
                        params![publication.publication_key.as_str()],
                    )
                    .map_err(map_sqlite_session_error)?;
                materialized.push(existing);
                continue;
            }
            ensure_event_id_available(&transaction, &publication)?;
            last_position = last_position.checked_add(1).ok_or_else(|| {
                SessionStoreError::Failed(
                    "durable host event position space is exhausted".to_string(),
                )
            })?;
            let position = u64::try_from(last_position).map_err(map_display_session_error)?;
            let record = DurableHostEventRecord::from_pending(position, publication.clone());
            let payload = serialize_json_record(&record)?;
            let (scope_kind, session_id, run_id) = scope_columns(&record.scope);
            transaction
                .execute(
                    "INSERT INTO host_event_records
                     (position, publication_key, event_id, scope_kind, session_id, run_id,
                      event_class, record, occurred_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        last_position,
                        record.publication_key.as_str(),
                        record.event_id,
                        scope_kind,
                        session_id,
                        run_id,
                        record.event_class.as_str(),
                        payload,
                        record.occurred_at.to_rfc3339(),
                    ],
                )
                .map_err(map_sqlite_session_error)?;
            transaction
                .execute(
                    "DELETE FROM host_event_publication_outbox WHERE publication_key = ?1",
                    params![publication.publication_key.as_str()],
                )
                .map_err(map_sqlite_session_error)?;
            materialized.push(record);
        }
        transaction
            .execute(
                "UPDATE host_event_log_state SET last_position = ?1 WHERE singleton = 1",
                params![last_position],
            )
            .map_err(map_sqlite_session_error)?;
        transaction.commit().map_err(map_sqlite_session_error)?;
        Ok(materialized)
    }

    pub(super) fn replay_host_events_sync(
        &self,
        query: DurableHostEventQuery,
    ) -> SessionStoreResult<DurableHostEventPage> {
        let query = DurableHostEventQuery::new(
            query.scope,
            query.event_classes,
            query.after_position,
            query.limit,
        )?;
        let fetch_limit = query.limit.saturating_add(1);
        let (sql, values) = filtered_event_sql(
            "SELECT record",
            &query.scope,
            &query.event_classes,
            query.after_position,
            Some(fetch_limit),
        )?;
        let connection = self.lock()?;
        let mut statement = connection.prepare(&sql).map_err(map_sqlite_session_error)?;
        let rows = statement
            .query_map(params_from_iter(values), |row| row.get::<_, String>(0))
            .map_err(map_sqlite_session_error)?;
        let mut records = collect_json_record_rows::<DurableHostEventRecord>(rows)?;
        let has_more = records.len() > query.limit;
        if has_more {
            records.pop();
        }
        let next_position = records
            .last()
            .map(|record| record.position)
            .or(query.after_position);
        Ok(DurableHostEventPage {
            records,
            next_position,
            has_more,
        })
    }

    pub(super) fn host_event_fence_sync(
        &self,
        scope: &DurableHostEventScope,
        event_classes: &[DurableHostEventClass],
    ) -> SessionStoreResult<Option<u64>> {
        let classes = event_classes.iter().copied().collect::<BTreeSet<_>>();
        let (sql, values) =
            filtered_event_sql("SELECT MAX(position)", scope, &classes, None, None)?;
        let connection = self.lock()?;
        let position = connection
            .query_row(&sql, params_from_iter(values), |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(map_sqlite_session_error)?;
        position
            .map(|position| u64::try_from(position).map_err(map_display_session_error))
            .transpose()
    }
}
