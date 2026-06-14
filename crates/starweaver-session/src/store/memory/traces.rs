use starweaver_core::{RunId, SessionId};

use crate::{
    approval::ApprovalStatus,
    error::{SessionStoreError, SessionStoreResult},
    trace::{CompactRunTrace, CompactSessionTrace},
};

use super::{run_key, run_key_label, store_failed, InMemorySessionStore};

impl InMemorySessionStore {
    pub(super) fn compact_run_trace_projection(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<CompactRunTrace> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        let run = inner
            .runs
            .get(&key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        let checkpoints = inner.checkpoints.get(&key).cloned().unwrap_or_default();
        let stream_cursor = inner
            .streams
            .get(&key)
            .and_then(|records| records.last())
            .map(|record| record.sequence);
        let approvals = inner.approvals.get(&key).map_or(0, |records| {
            records
                .iter()
                .filter(|record| record.status == ApprovalStatus::Pending)
                .count()
        });
        let deferred_tools = inner.deferred_tools.get(&key).map_or(0, Vec::len);
        Ok(CompactRunTrace {
            session_id: Some(session_id.clone()),
            run_id: Some(run_id.clone()),
            status: run.status,
            checkpoints: checkpoints
                .iter()
                .map(|checkpoint| checkpoint.checkpoint_id.clone())
                .collect(),
            approvals,
            deferred_tools,
            latest_checkpoint: checkpoints
                .last()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            stream_cursor,
            stream_cursors: run.stream_cursors.clone(),
            output_preview: run.output_preview.clone(),
            trace_context: run.trace_context.clone(),
            updated_at: Some(run.updated_at),
            metadata: run.metadata.clone(),
        })
    }

    pub(super) fn compact_session_trace_projection(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<CompactSessionTrace> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        let mut runs = inner
            .runs
            .iter()
            .filter(|((stored_session_id, _run_id), _run)| stored_session_id == session_id)
            .map(|(_key, run)| run.clone())
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.created_at);
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
    }
}
