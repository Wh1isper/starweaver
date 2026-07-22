use chrono::Utc;
use starweaver_context::AgentCheckpoint;
use starweaver_core::{RunId, SessionId};

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::CheckpointRef,
};

use super::{InMemorySessionStore, advance_run_revision, run_key, run_key_label, store_failed};

impl InMemorySessionStore {
    pub(super) fn append_checkpoint_record(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        self.append_checkpoint_record_with_revision(session_id, checkpoint, true)
    }

    pub(super) fn append_checkpoint_record_with_revision(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
        advance_revision: bool,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, &checkpoint.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id,
                &checkpoint.run_id,
            )));
        }
        let checkpoints = inner.checkpoints.entry(key.clone()).or_default();
        if let Some(existing) = checkpoints
            .iter()
            .find(|existing| existing.checkpoint_id == checkpoint.checkpoint_id)
        {
            if existing == &checkpoint {
                return Ok(());
            }
            return Err(SessionStoreError::Failed(format!(
                "checkpoint conflict for session {} run {} checkpoint {}",
                session_id.as_str(),
                checkpoint.run_id.as_str(),
                checkpoint.checkpoint_id.as_str()
            )));
        }
        checkpoints.push(checkpoint.clone());
        if let Some(run) = inner.runs.get_mut(&key) {
            run.latest_checkpoint = Some(CheckpointRef {
                checkpoint_id: checkpoint.checkpoint_id,
                run_id: checkpoint.run_id,
                sequence: checkpoint.run_step,
                node: format!("{:?}", checkpoint.node),
                storage_ref: None,
                stream_cursor: checkpoint.resume.cursor.stream_cursor,
                created_at: Utc::now(),
                metadata: checkpoint.metadata,
            });
            if advance_revision {
                advance_run_revision(run)?;
            }
            run.updated_at = Utc::now();
        }
        Ok(())
    }

    pub(super) fn load_checkpoint_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentCheckpoint>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .checkpoints
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }
}
