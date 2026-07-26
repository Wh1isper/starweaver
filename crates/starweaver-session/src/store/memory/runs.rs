use chrono::Utc;
use starweaver_core::{RunId, SessionId};

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::{RunRecord, RunStatus, RunTerminalError},
    store::{RunPage, RunPageKey, RunPageQuery},
};

use super::{
    InMemorySessionStore, advance_run_revision, insert_prepared_run_record, prepare_new_run_record,
    run_key, run_key_label, store_failed,
};

impl InMemorySessionStore {
    pub(super) fn append_run_record(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        run.validate_new_write().map_err(|error| {
            SessionStoreError::Failed(format!(
                "invalid run state for {}: {error}",
                run.run_id.as_str()
            ))
        })?;
        let mut inner = self.inner.lock().map_err(store_failed)?;
        run.updated_at = Utc::now();
        if !inner.sessions.contains_key(&run.session_id) {
            return Err(SessionStoreError::NotFound(
                run.session_id.as_str().to_string(),
            ));
        }
        let key = run_key(&run.session_id, &run.run_id);
        let existing = if let Some(persisted) = inner.runs.get(&key) {
            if run.sequence_no != 0 && run.sequence_no != persisted.sequence_no {
                return Err(SessionStoreError::Failed(format!(
                    "run sequence is immutable for session {} and run {}: persisted {}, received {}",
                    run.session_id.as_str(),
                    run.run_id.as_str(),
                    persisted.sequence_no,
                    run.sequence_no
                )));
            }
            run.sequence_no = persisted.sequence_no;
            run.revision = persisted.revision;
            advance_run_revision(&mut run)?;
            true
        } else {
            run.revision = 1;
            prepare_new_run_record(&inner, &mut run)?;
            false
        };
        if existing {
            inner.runs.insert(key, run.clone());
        } else {
            insert_prepared_run_record(&mut inner, &run);
        }
        if let Some(session) = inner.sessions.get_mut(&run.session_id) {
            session.head_run_id = Some(run.run_id.clone());
            if run.status.is_active() {
                session.active_run_id = Some(run.run_id.clone());
            } else {
                if run.status == RunStatus::Completed {
                    session.head_success_run_id = Some(run.run_id.clone());
                }
                if session.active_run_id.as_ref() == Some(&run.run_id) {
                    session.active_run_id = None;
                }
            }
            session.revision = session.revision.saturating_add(1);
            session.updated_at = run.updated_at;
        }
        Ok(())
    }

    pub(super) fn load_run_record(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<RunRecord> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        inner
            .runs
            .get(&key)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))
    }

    pub(super) fn list_run_records(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<Vec<RunRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let Some(sequences) = inner.run_sequences.get(session_id) else {
            return Ok(Vec::new());
        };
        sequences
            .values()
            .map(|run_id| {
                inner
                    .runs
                    .get(&run_key(session_id, run_id))
                    .cloned()
                    .ok_or_else(|| {
                        SessionStoreError::Failed("run sequence index is inconsistent".to_string())
                    })
            })
            .collect()
    }

    pub(super) fn list_recent_run_records(
        &self,
        session_id: &SessionId,
        limit: usize,
    ) -> SessionStoreResult<Vec<RunRecord>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let inner = self.inner.lock().map_err(store_failed)?;
        let Some(sequences) = inner.run_sequences.get(session_id) else {
            return Ok(Vec::new());
        };
        let mut runs = sequences
            .values()
            .rev()
            .take(limit)
            .map(|run_id| {
                inner
                    .runs
                    .get(&run_key(session_id, run_id))
                    .cloned()
                    .ok_or_else(|| {
                        SessionStoreError::Failed("run sequence index is inconsistent".to_string())
                    })
            })
            .collect::<SessionStoreResult<Vec<_>>>()?;
        runs.reverse();
        Ok(runs)
    }

    pub(super) fn list_run_record_page(
        &self,
        session_id: &SessionId,
        query: &RunPageQuery,
    ) -> SessionStoreResult<RunPage> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let before = query.before().map(|key| key.sequence_no);
        let fetch_limit = query.limit().saturating_add(1);
        let mut runs = Vec::with_capacity(fetch_limit);
        if let Some(sequences) = inner.run_sequences.get(session_id) {
            let run_ids = before.map_or_else(
                || {
                    sequences
                        .values()
                        .rev()
                        .take(fetch_limit)
                        .collect::<Vec<_>>()
                },
                |boundary| {
                    sequences
                        .range(..boundary)
                        .rev()
                        .map(|(_sequence, run_id)| run_id)
                        .take(fetch_limit)
                        .collect::<Vec<_>>()
                },
            );
            for run_id in run_ids {
                let run = inner
                    .runs
                    .get(&run_key(session_id, run_id))
                    .cloned()
                    .ok_or_else(|| {
                        SessionStoreError::Failed("run sequence index is inconsistent".to_string())
                    })?;
                runs.push(run);
            }
        }
        let has_more = runs.len() > query.limit();
        runs.truncate(query.limit());
        let next_key = runs
            .last()
            .map(RunPageKey::from_run)
            .or_else(|| query.before().cloned());
        Ok(RunPage {
            runs,
            next_key,
            has_more,
        })
    }

    pub(super) fn set_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
        terminal_error: Option<RunTerminalError>,
    ) -> SessionStoreResult<()> {
        self.update_run_record(session_id, run_id, |run| {
            run.status = status;
            run.output_preview = output_preview;
            run.terminal_error = terminal_error;
        })
    }

    pub(super) fn set_legacy_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        self.update_run_record(session_id, run_id, |run| {
            run.apply_legacy_status_update(status, output_preview);
        })
    }

    fn update_run_record(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        update: impl FnOnce(&mut RunRecord),
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let mut run = inner
            .runs
            .get(&key)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        let previous = run.clone();
        update(&mut run);
        run.validate_new_write().map_err(|error| {
            SessionStoreError::Failed(format!(
                "invalid run state for {}: {error}",
                run.run_id.as_str()
            ))
        })?;
        if run == previous {
            return Ok(());
        }
        run.revision = previous.revision;
        advance_run_revision(&mut run)?;
        run.updated_at = updated_at;
        let status = run.status;
        inner.runs.insert(key, run);
        if let Some(session) = inner.sessions.get_mut(session_id) {
            session.head_run_id = Some(run_id.clone());
            if status.is_active() {
                session.active_run_id = Some(run_id.clone());
            } else {
                if status == RunStatus::Completed {
                    session.head_success_run_id = Some(run_id.clone());
                }
                if session.active_run_id.as_ref() == Some(run_id) {
                    session.active_run_id = None;
                }
            }
            session.revision = session.revision.saturating_add(1);
            session.updated_at = updated_at;
        }
        Ok(())
    }
}
