use chrono::Utc;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentCheckpoint;

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::CheckpointRef,
};

use super::{run_key, run_key_label, store_failed, InMemorySessionStore};

impl InMemorySessionStore {
    pub(super) fn append_checkpoint_record(
        &self,
        session_id: &SessionId,
        checkpoint: AgentCheckpoint,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, &checkpoint.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id,
                &checkpoint.run_id,
            )));
        }
        inner
            .checkpoints
            .entry(key.clone())
            .or_default()
            .push(checkpoint.clone());
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
