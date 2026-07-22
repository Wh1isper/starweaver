use chrono::Utc;
use starweaver_core::{RunId, SessionId};
use starweaver_stream::{AgentStreamRecord, ReplayCursor, ReplayScope};

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::StreamCursorRef,
};

use super::{InMemorySessionStore, advance_run_revision, run_key, run_key_label, store_failed};

impl InMemorySessionStore {
    pub(super) fn append_stream_record_batch(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
    ) -> SessionStoreResult<()> {
        self.append_stream_record_batch_with_revision(session_id, run_id, records, true)
    }

    pub(super) fn append_stream_record_batch_with_revision(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        records: Vec<AgentStreamRecord>,
        advance_revision: bool,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                session_id, run_id,
            )));
        }
        let stream = inner.streams.entry(key.clone()).or_default();
        for (index, record) in records.iter().enumerate() {
            let existing = stream
                .iter()
                .find(|existing| existing.sequence == record.sequence)
                .or_else(|| {
                    records[..index]
                        .iter()
                        .find(|existing| existing.sequence == record.sequence)
                });
            if existing.is_some_and(|existing| existing != record) {
                return Err(SessionStoreError::Failed(format!(
                    "stream record conflict for session {} run {} at sequence {}",
                    session_id.as_str(),
                    run_id.as_str(),
                    record.sequence
                )));
            }
        }
        let mut inserted_any = false;
        for record in records {
            if stream
                .iter()
                .all(|existing| existing.sequence != record.sequence)
            {
                stream.push(record);
                inserted_any = true;
            }
        }
        stream.sort_by_key(|record| record.sequence);
        let last_sequence = stream.last().map(|record| record.sequence);
        if let Some(run) = inner.runs.get_mut(&key) {
            if let Some(sequence) = last_sequence {
                let cursor = StreamCursorRef::new(ReplayCursor::raw_runtime(
                    ReplayScope::run(run_id.as_str()),
                    sequence,
                ));
                run.stream_cursors
                    .retain(|existing| !existing.same_stream(&cursor));
                run.stream_cursors.push(cursor);
            }
            if inserted_any {
                if advance_revision {
                    advance_run_revision(run)?;
                }
                run.updated_at = Utc::now();
            }
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
        self.save_stream_cursor_ref_with_revision(session_id, run_id, cursor, true)
    }

    pub(super) fn save_stream_cursor_ref_with_revision(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        cursor: StreamCursorRef,
        advance_revision: bool,
    ) -> SessionStoreResult<()> {
        cursor
            .validate_for_run(run_id)
            .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let run_key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let existing_run = inner
            .runs
            .get(&run_key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        for existing in existing_run.stream_cursors.iter().chain(
            inner
                .sessions
                .get(session_id)
                .into_iter()
                .flat_map(|session| &session.stream_cursors),
        ) {
            cursor
                .validate_progression(existing)
                .map_err(|error| SessionStoreError::Failed(error.to_string()))?;
        }
        if existing_run
            .stream_cursors
            .iter()
            .any(|existing| existing == &cursor)
        {
            return Ok(());
        }
        let run = inner
            .runs
            .get_mut(&run_key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        run.stream_cursors
            .retain(|existing| !existing.same_stream(&cursor));
        run.stream_cursors.push(cursor.clone());
        if advance_revision {
            advance_run_revision(run)?;
        }
        run.updated_at = updated_at;
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session
                .stream_cursors
                .retain(|existing| !existing.same_stream(&cursor));
            session.stream_cursors.push(cursor);
            session.updated_at = updated_at;
        }
        Ok(())
    }
}
