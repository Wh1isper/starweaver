//! `StreamArchive` adapter over the CLI local store.

use async_trait::async_trait;
use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;
use starweaver_stream::{
    DisplayMessage, ReplayCursor, ReplayError, ReplayResult, ReplayScope, ReplaySnapshot,
    StreamArchive,
};

use super::{
    DisplayReplayWindow, LocalStore,
    db::insert_raw_stream_records_tx,
    db::{insert_display_messages_for_run_tx, insert_display_messages_tx},
};
use crate::{CliResult, config::CliConfig};

/// Shared stream archive adapter backed by the CLI local `SQLite` store.
#[derive(Clone, Debug)]
pub struct LocalStreamArchive {
    config: CliConfig,
}

enum ParsedReplayScope<'a> {
    Run(&'a str),
    Session(&'a str),
}

enum DisplayAppendTarget {
    Run((SessionId, RunId)),
    Session,
}

impl LocalStreamArchive {
    /// Create a local stream archive adapter from resolved CLI config.
    #[must_use]
    pub const fn new(config: CliConfig) -> Self {
        Self { config }
    }

    fn open_store(&self) -> ReplayResult<LocalStore> {
        LocalStore::open(&self.config).map_err(replay_failed)
    }

    /// Replay display messages as scoped replay events for local RPC and TUI hosts.
    pub fn replay_display_window(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        cursor: Option<&ReplayCursor>,
    ) -> CliResult<DisplayReplayWindow> {
        LocalStore::open(&self.config)?.replay_display_window(session_id, run_id, cursor)
    }
}

#[async_trait]
impl StreamArchive for LocalStreamArchive {
    async fn append_raw_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> ReplayResult<()> {
        let mut store = self.open_store()?;
        let run = store
            .load_run(session_id.as_str(), run_id.as_str())
            .map_err(replay_failed)?;
        let tx = store
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(replay_failed)?;
        insert_raw_stream_records_tx(&tx, &run, &records).map_err(replay_failed)?;
        tx.commit().map_err(replay_failed)
    }

    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        let scope = ReplayScope::run(run_id.as_str());
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(&scope)?;
        }
        let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
        let store = self.open_store()?;
        let mut stmt = store
            .conn
            .prepare(
                r"
                SELECT record_json
                FROM raw_stream_records
                WHERE session_id = ?1 AND run_id = ?2 AND sequence_no >= ?3
                ORDER BY sequence_no ASC
                ",
            )
            .map_err(replay_failed)?;
        let rows = stmt
            .query_map(
                params![
                    session_id.as_str(),
                    run_id.as_str(),
                    i64::try_from(after).map_err(replay_failed)?
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(replay_failed)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(replay_failed)?
            .into_iter()
            .map(|json| serde_json::from_str(&json).map_err(replay_failed))
            .collect()
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let mut store = self.open_store()?;
        let append_target = match parse_scope(&scope)? {
            ParsedReplayScope::Run(run_id) => {
                let storage_run_ref = storage_run_ref_for_scope(&store, run_id)?;
                validate_run_scoped_display_messages(&storage_run_ref.0, &messages)?;
                DisplayAppendTarget::Run(storage_run_ref)
            }
            ParsedReplayScope::Session(session_id) => {
                validate_session_scoped_display_messages(&store, session_id, &messages)?;
                DisplayAppendTarget::Session
            }
        };
        let tx = store
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(replay_failed)?;
        match append_target {
            DisplayAppendTarget::Run((storage_session_id, storage_run_id)) => {
                insert_display_messages_for_run_tx(
                    &tx,
                    &storage_session_id,
                    &storage_run_id,
                    &messages,
                )
                .map_err(replay_failed)?;
            }
            DisplayAppendTarget::Session => {
                insert_display_messages_tx(&tx, &messages).map_err(replay_failed)?;
            }
        }
        tx.commit().map_err(replay_failed)
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        if let Some(cursor) = cursor.as_ref() {
            cursor.validate_scope(scope)?;
        }
        let store = self.open_store()?;
        match parse_scope(scope)? {
            ParsedReplayScope::Run(run_id) => {
                let (session_id, run_id) = storage_run_ref_for_scope(&store, run_id)?;
                replay_run_display(&store, &session_id, &run_id, cursor.as_ref())
            }
            ParsedReplayScope::Session(session_id) => {
                replay_session_display(&store, session_id, cursor.as_ref())
            }
        }
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        let store = self.open_store()?;
        store
            .conn
            .execute(
                "INSERT OR REPLACE INTO replay_snapshots (scope, snapshot_json, updated_at)
                 VALUES (?1, ?2, ?3)",
                params![
                    scope.as_str(),
                    serde_json::to_string(&snapshot).map_err(replay_failed)?,
                    Utc::now().to_rfc3339()
                ],
            )
            .map_err(replay_failed)?;
        Ok(())
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        let store = self.open_store()?;
        store
            .conn
            .query_row(
                "SELECT snapshot_json FROM replay_snapshots WHERE scope = ?1",
                params![scope.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(replay_failed)?
            .map(|json| serde_json::from_str(&json).map_err(replay_failed))
            .transpose()
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        let store = self.open_store()?;
        match parse_scope(scope)? {
            ParsedReplayScope::Run(run_id) => {
                let (session_id, run_id) = storage_run_ref_for_scope(&store, run_id)?;
                run_cursor_range(&store, scope, &session_id, &run_id)
            }
            ParsedReplayScope::Session(session_id) => {
                session_cursor_range(&store, scope, session_id)
            }
        }
    }
}

fn storage_run_ref_for_scope(store: &LocalStore, run_id: &str) -> ReplayResult<(SessionId, RunId)> {
    let mut stmt = store
        .conn
        .prepare("SELECT session_id FROM runs WHERE run_id = ?1 ORDER BY updated_at DESC LIMIT 2")
        .map_err(replay_failed)?;
    let session_ids = stmt
        .query_map(params![run_id], |row| row.get::<_, String>(0))
        .map_err(replay_failed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(replay_failed)?;
    match session_ids.as_slice() {
        [] => Err(ReplayError::NotFound(format!("run:{run_id}"))),
        [session_id] => Ok((
            SessionId::from_string(session_id.clone()),
            RunId::from_string(run_id.to_string()),
        )),
        _ => Err(ReplayError::Failed(format!(
            "run scope run:{run_id} is ambiguous across multiple sessions"
        ))),
    }
}

fn validate_run_scoped_display_messages(
    storage_session_id: &SessionId,
    messages: &[DisplayMessage],
) -> ReplayResult<()> {
    for (index, message) in messages.iter().enumerate() {
        if message.session_id.as_str() != storage_session_id.as_str() {
            return Err(ReplayError::Failed(format!(
                "display message at index {index} has session_id {}, but run scope belongs to session_id {}",
                message.session_id.as_str(),
                storage_session_id.as_str()
            )));
        }
    }
    Ok(())
}

fn validate_session_scoped_display_messages(
    store: &LocalStore,
    session_id: &str,
    messages: &[DisplayMessage],
) -> ReplayResult<()> {
    for (index, message) in messages.iter().enumerate() {
        if message.session_id.as_str() != session_id {
            return Err(ReplayError::Failed(format!(
                "display message at index {index} has session_id {}, but session scope is session:{session_id}",
                message.session_id.as_str()
            )));
        }
        let run_exists = store
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE session_id = ?1 AND run_id = ?2)",
                params![session_id, message.run_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(replay_failed)?
            != 0;
        if !run_exists {
            return Err(ReplayError::Failed(format!(
                "display message at index {index} has run_id {}, which is not a run in session scope session:{session_id}",
                message.run_id.as_str()
            )));
        }
    }
    Ok(())
}

fn replay_run_display(
    store: &LocalStore,
    session_id: &SessionId,
    run_id: &RunId,
    cursor: Option<&ReplayCursor>,
) -> ReplayResult<Vec<DisplayMessage>> {
    let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
    let mut stmt = store
        .conn
        .prepare(
            r"
            SELECT message_json
            FROM display_messages
            WHERE session_id = ?1 AND run_id = ?2 AND sequence_no >= ?3
            ORDER BY sequence_no ASC
            ",
        )
        .map_err(replay_failed)?;
    let rows = stmt
        .query_map(
            params![
                session_id.as_str(),
                run_id.as_str(),
                i64::try_from(after).map_err(replay_failed)?
            ],
            |row| row.get::<_, String>(0),
        )
        .map_err(replay_failed)?;
    collect_display_messages(rows)
}

fn replay_session_display(
    store: &LocalStore,
    session_id: &str,
    cursor: Option<&ReplayCursor>,
) -> ReplayResult<Vec<DisplayMessage>> {
    let after = cursor.map_or(0, |cursor| cursor.sequence.saturating_add(1));
    let mut stmt = store
        .conn
        .prepare(
            r"
            SELECT dm.message_json
            FROM display_messages dm
            JOIN runs r ON r.session_id = dm.session_id AND r.run_id = dm.run_id
            WHERE dm.session_id = ?1
            ORDER BY r.sequence_no ASC, dm.sequence_no ASC
            ",
        )
        .map_err(replay_failed)?;
    let rows = stmt
        .query_map(params![session_id], |row| row.get::<_, String>(0))
        .map_err(replay_failed)?;
    let messages = collect_display_messages(rows)?;
    Ok(messages
        .into_iter()
        .enumerate()
        .filter_map(|(sequence, message)| (sequence >= after).then_some(message))
        .collect())
}

fn collect_display_messages(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<String>>,
) -> ReplayResult<Vec<DisplayMessage>> {
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(replay_failed)?
        .into_iter()
        .map(|json| serde_json::from_str(&json).map_err(replay_failed))
        .collect()
}

fn run_cursor_range(
    store: &LocalStore,
    scope: &ReplayScope,
    session_id: &SessionId,
    run_id: &RunId,
) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
    let range = store
        .conn
        .query_row(
            "SELECT MIN(sequence_no), MAX(sequence_no) FROM display_messages WHERE session_id = ?1 AND run_id = ?2",
            params![session_id.as_str(), run_id.as_str()],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(replay_failed)?;
    let (Some(first), Some(last)) = range else {
        return Ok(None);
    };
    Ok(Some((
        ReplayCursor::new(
            scope.clone(),
            usize::try_from(first).map_err(replay_failed)?,
        ),
        ReplayCursor::new(scope.clone(), usize::try_from(last).map_err(replay_failed)?),
    )))
}

fn session_cursor_range(
    store: &LocalStore,
    scope: &ReplayScope,
    session_id: &str,
) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
    let count = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM display_messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(replay_failed)?;
    let count = usize::try_from(count).map_err(replay_failed)?;
    if count == 0 {
        return Ok(None);
    }
    Ok(Some((
        ReplayCursor::new(scope.clone(), 0),
        ReplayCursor::new(scope.clone(), count.saturating_sub(1)),
    )))
}

fn parse_scope(scope: &ReplayScope) -> ReplayResult<ParsedReplayScope<'_>> {
    if let Some(run_id) = scope.as_str().strip_prefix("run:") {
        return Ok(ParsedReplayScope::Run(run_id));
    }
    if let Some(session_id) = scope.as_str().strip_prefix("session:") {
        return Ok(ParsedReplayScope::Session(session_id));
    }
    Err(ReplayError::InvalidCursor(format!(
        "unsupported replay scope {}",
        scope.as_str()
    )))
}

fn replay_failed(error: impl std::fmt::Display) -> ReplayError {
    ReplayError::Failed(error.to_string())
}
