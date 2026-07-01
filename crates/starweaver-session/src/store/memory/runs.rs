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
        inner
            .runs
            .insert(run_key(&run.session_id, &run.run_id), run.clone());
        if let Some(session) = inner.sessions.get_mut(&run.session_id) {
            session.head_run_id = Some(run.run_id.clone());
            if matches!(
                run.status,
                RunStatus::Queued | RunStatus::Running | RunStatus::Waiting
            ) {
                session.active_run_id = Some(run.run_id.clone());
            }
            if run.status == RunStatus::Completed {
                session.head_success_run_id = Some(run.run_id.clone());
                if session.active_run_id.as_ref() == Some(&run.run_id) {
                    session.active_run_id = None;
                }
            }
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
        runs.sort_by_key(|run| run.created_at);
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
            match status {
                RunStatus::Queued | RunStatus::Running | RunStatus::Waiting => {
                    session.active_run_id = Some(run_id.clone());
                }
                RunStatus::Completed => {
                    session.head_success_run_id = Some(run_id.clone());
                    if session.active_run_id.as_ref() == Some(run_id) {
                        session.active_run_id = None;
                    }
                }
                RunStatus::Failed | RunStatus::Cancelled => {
                    if session.active_run_id.as_ref() == Some(run_id) {
                        session.active_run_id = None;
                    }
                }
            }
            session.updated_at = updated_at;
        }
        Ok(())
    }
}
