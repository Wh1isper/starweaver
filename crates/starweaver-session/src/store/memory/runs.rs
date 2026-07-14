use chrono::Utc;
use starweaver_core::{RunId, SessionId};

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::{RunRecord, RunStatus},
};

use super::{InMemorySessionStore, run_key, run_key_label, store_failed};

impl InMemorySessionStore {
    pub(super) fn append_run_record(&self, mut run: RunRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        run.updated_at = Utc::now();
        if !inner.sessions.contains_key(&run.session_id) {
            return Err(SessionStoreError::NotFound(
                run.session_id.as_str().to_string(),
            ));
        }
        let key = run_key(&run.session_id, &run.run_id);
        if let Some(persisted) = inner.runs.get(&key) {
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
        } else if run.sequence_no == 0 {
            run.sequence_no = inner
                .runs
                .values()
                .filter(|persisted| persisted.session_id == run.session_id)
                .map(|persisted| persisted.sequence_no)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or_else(|| SessionStoreError::Failed("run sequence overflow".to_string()))?;
        } else if inner.runs.values().any(|persisted| {
            persisted.session_id == run.session_id && persisted.sequence_no == run.sequence_no
        }) {
            return Err(SessionStoreError::Failed(format!(
                "run sequence conflict for session {} at sequence {}",
                run.session_id.as_str(),
                run.sequence_no
            )));
        }
        inner.runs.insert(key, run.clone());
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
        let mut runs = inner
            .runs
            .iter()
            .filter(|((stored_session_id, _run_id), _run)| stored_session_id == session_id)
            .map(|(_key, run)| run.clone())
            .collect::<Vec<_>>();
        runs.sort_by_key(|run| run.sequence_no);
        Ok(runs)
    }

    pub(super) fn set_run_status(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        status: RunStatus,
        output_preview: Option<String>,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(session_id, run_id);
        let updated_at = Utc::now();
        let run = inner
            .runs
            .get_mut(&key)
            .ok_or_else(|| SessionStoreError::NotFound(run_key_label(session_id, run_id)))?;
        run.status = status;
        run.output_preview = output_preview;
        run.updated_at = updated_at;
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
