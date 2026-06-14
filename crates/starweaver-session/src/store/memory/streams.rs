use chrono::Utc;
use starweaver_core::{RunId, SessionId};
use starweaver_runtime::AgentStreamRecord;

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::StreamCursorRef,
};

use super::{run_key, run_key_label, store_failed, InMemorySessionStore};

impl InMemorySessionStore {
    pub(super) fn append_stream_record_batch(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id, run_id,
            )));
        }
        let stream = inner.streams.entry(key.clone()).or_default();
        for record in records {
            if stream
                .iter()
                .all(|existing| existing.sequence != record.sequence)
            {
                stream.push(record);
            }
        }
        stream.sort_by_key(|record| record.sequence);
        let last_sequence = stream.last().map(|record| record.sequence);
        if let Some(run) = inner.runs.get_mut(&key) {
            if let Some(sequence) = last_sequence {
                let cursor = StreamCursorRef::new(
                    "raw_runtime",
                    format!("run:{}", run_id.as_str()),
                    sequence,
                );
                run.stream_cursors
                    .retain(|existing| existing.family != cursor.family);
                run.stream_cursors.push(cursor);
            }
            run.updated_at = Utc::now();
        }
        Ok(())
    }

    pub(super) fn replay_stream_record_batch(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<AgentStreamRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .streams
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    pub(super) fn save_stream_cursor_ref(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let run_key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let run = inner
            .runs
            .get_mut(&run_key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        run.stream_cursors
            .retain(|existing| existing.family != cursor.family || existing.scope != cursor.scope);
        run.stream_cursors.push(cursor.clone());
        run.updated_at = updated_at;
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session.stream_cursors.retain(|existing| {
                existing.family != cursor.family || existing.scope != cursor.scope
            });
            session.stream_cursors.push(cursor);
            session.updated_at = updated_at;
        }
        Ok(())
    }
}
