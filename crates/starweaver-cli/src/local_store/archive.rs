//! CLI adapter over the shared `SQLite` stream archive.

use async_trait::async_trait;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;
use starweaver_storage::{SqliteStorage, SqliteStreamArchive};
use starweaver_stream::{
    DisplayMessage, ReplayCursor, ReplayCursorFamily, ReplayError, ReplayResult, ReplayScope,
    ReplaySnapshot, StreamArchive,
};

use super::{DisplayReplayWindow, LocalStore};
use crate::{CliResult, config::CliConfig};

async fn run_blocking<T, F>(operation: F) -> ReplayResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> ReplayResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|error| ReplayError::Failed(format!("blocking archive task failed: {error}")))?
}

/// Stream archive bound to a resolved CLI database path.
#[derive(Clone, Debug)]
pub struct LocalStreamArchive {
    config: CliConfig,
    storage: SqliteStorage,
    archive: SqliteStreamArchive,
}

impl LocalStreamArchive {
    /// Open a CLI archive adapter before it is used by async runtime code.
    pub fn new(config: CliConfig) -> ReplayResult<Self> {
        crate::config::ensure_config_dirs(&config)
            .map_err(|error| ReplayError::Failed(error.to_string()))?;
        let storage = SqliteStorage::open(&config.database_path)
            .map_err(|error| ReplayError::Failed(error.to_string()))?;
        let archive = storage.stream_archive();
        Ok(Self {
            config,
            storage,
            archive,
        })
    }

    fn validate_display_messages(
        storage: &SqliteStorage,
        scope: &ReplayScope,
        messages: &[DisplayMessage],
    ) -> ReplayResult<()> {
        if let Some(run_id) = scope.as_str().strip_prefix("run:") {
            let sessions = storage
                .list_sessions()
                .map_err(|error| ReplayError::Failed(error.to_string()))?;
            let mut owners = Vec::new();
            for session in sessions {
                let owns_run = storage
                    .list_runs(&session.session_id)
                    .map_err(|error| ReplayError::Failed(error.to_string()))?
                    .iter()
                    .any(|run| run.run_id.as_str() == run_id);
                if owns_run {
                    owners.push(session.session_id);
                }
            }
            let owner = match owners.as_slice() {
                [owner] => owner,
                [] => return Err(ReplayError::NotFound(scope.as_str().to_string())),
                _ => {
                    return Err(ReplayError::Failed(format!(
                        "run scope {} is ambiguous across sessions",
                        scope.as_str()
                    )));
                }
            };
            for (index, message) in messages.iter().enumerate() {
                if message.session_id != *owner {
                    return Err(ReplayError::Failed(format!(
                        "display message at index {index} has session_id {}, but run scope belongs to session_id {}",
                        message.session_id.as_str(),
                        owner.as_str()
                    )));
                }
            }
            return Ok(());
        }
        if let Some(session_id) = scope.as_str().strip_prefix("session:") {
            let session_id = SessionId::from_string(session_id);
            let run_ids = storage
                .list_runs(&session_id)
                .map_err(|error| ReplayError::Failed(error.to_string()))?
                .into_iter()
                .map(|run| run.run_id)
                .collect::<Vec<_>>();
            for (index, message) in messages.iter().enumerate() {
                if message.session_id != session_id {
                    return Err(ReplayError::Failed(format!(
                        "display message at index {index} has session_id {}, but session scope is session:{}",
                        message.session_id.as_str(),
                        session_id.as_str()
                    )));
                }
                if !run_ids.contains(&message.run_id) {
                    return Err(ReplayError::Failed(format!(
                        "display message at index {index} has run_id {}, which is not a run in session scope session:{}",
                        message.run_id.as_str(),
                        session_id.as_str()
                    )));
                }
            }
            return Ok(());
        }
        Err(ReplayError::InvalidCursor(format!(
            "unsupported replay scope {}",
            scope.as_str()
        )))
    }

    /// Replay display messages as a CLI projection window.
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
        if records.is_empty() {
            return Ok(());
        }
        self.archive
            .append_raw_records(session_id, run_id, records)
            .await
    }

    async fn replay_raw_after(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<AgentStreamRecord>> {
        self.archive
            .replay_raw_after(session_id, run_id, cursor)
            .await
    }

    async fn append_display_messages(
        &self,
        scope: ReplayScope,
        messages: Vec<DisplayMessage>,
    ) -> ReplayResult<()> {
        if messages.is_empty() {
            return Ok(());
        }
        let storage = self.storage.clone();
        let validation_scope = scope.clone();
        let validation_messages = messages.clone();
        run_blocking(move || {
            Self::validate_display_messages(&storage, &validation_scope, &validation_messages)
        })
        .await?;
        self.archive.append_display_messages(scope, messages).await
    }

    async fn replay_display_after(
        &self,
        scope: &ReplayScope,
        cursor: Option<ReplayCursor>,
    ) -> ReplayResult<Vec<DisplayMessage>> {
        if let Some(session_id) = scope.as_str().strip_prefix("session:") {
            if let Some(cursor) = cursor.as_ref() {
                cursor.validate(ReplayCursorFamily::Display, scope)?;
            }
            let session_id = SessionId::from_string(session_id);
            let after = cursor.as_ref().map(|cursor| cursor.sequence);
            let storage = self.storage.clone();
            return run_blocking(move || {
                storage
                    .load_display_messages(&session_id, None, after)
                    .map_err(|error| ReplayError::Failed(error.to_string()))
            })
            .await;
        }
        self.archive.replay_display_after(scope, cursor).await
    }

    async fn append_snapshot(
        &self,
        scope: ReplayScope,
        snapshot: ReplaySnapshot,
    ) -> ReplayResult<()> {
        self.archive.append_snapshot(scope, snapshot).await
    }

    async fn latest_snapshot(&self, scope: &ReplayScope) -> ReplayResult<Option<ReplaySnapshot>> {
        self.archive.latest_snapshot(scope).await
    }

    async fn cursor_range(
        &self,
        scope: &ReplayScope,
    ) -> ReplayResult<Option<(ReplayCursor, ReplayCursor)>> {
        if let Some(session_id) = scope.as_str().strip_prefix("session:") {
            let session_id = SessionId::from_string(session_id);
            let storage = self.storage.clone();
            let message_count = run_blocking(move || {
                storage
                    .load_display_messages(&session_id, None, None)
                    .map(|messages| messages.len())
                    .map_err(|error| ReplayError::Failed(error.to_string()))
            })
            .await?;
            let Some(last_sequence) = message_count.checked_sub(1) else {
                return Ok(None);
            };
            return Ok(Some((
                ReplayCursor::display(scope.clone(), 0),
                ReplayCursor::display(scope.clone(), last_sequence),
            )));
        }
        self.archive.cursor_range(scope).await
    }
}
