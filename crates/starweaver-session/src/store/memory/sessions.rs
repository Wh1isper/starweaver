use chrono::Utc;
use starweaver_context::ResumableState;
use starweaver_core::SessionId;

use crate::{
    error::{SessionStoreError, SessionStoreResult},
    records::{EnvironmentStateRef, SessionRecord, SessionStatus},
    store::SessionFilter,
};

use super::{store_failed, InMemorySessionStore};

impl InMemorySessionStore {
    pub(super) fn save_session_record(&self, mut session: SessionRecord) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        session.updated_at = Utc::now();
        inner.sessions.insert(session.session_id.clone(), session);
        Ok(())
    }

    pub(super) fn load_session_record(
        &self,
        session_id: &SessionId,
    ) -> SessionStoreResult<SessionRecord> {
        let inner = self.inner.lock().map_err(store_failed)?;
        inner
            .sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))
    }

    #[allow(clippy::needless_pass_by_value)]
    pub(super) fn list_session_records(
        &self,
        filter: SessionFilter,
    ) -> SessionStoreResult<Vec<SessionRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        let mut sessions = inner
            .sessions
            .values()
            .filter(|session| filter.status.is_none_or(|status| session.status == status))
            .filter(|session| {
                filter
                    .profile
                    .as_ref()
                    .is_none_or(|profile| session.profile.as_ref() == Some(profile))
            })
            .filter(|session| {
                filter
                    .workspace
                    .as_ref()
                    .is_none_or(|workspace| session.workspace.as_ref() == Some(workspace))
            })
            .cloned()
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at));
        if let Some(limit) = filter.limit {
            sessions.truncate(limit);
        }
        Ok(sessions)
    }

    pub(super) fn set_session_status(
        &self,
        session_id: &SessionId,
        status: SessionStatus,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.status = status;
        session.updated_at = Utc::now();
        Ok(())
    }

    pub(super) fn save_context_state_snapshot(
        &self,
        session_id: &SessionId,
        state: ResumableState,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.state = state;
        session.updated_at = Utc::now();
        Ok(())
    }

    pub(super) fn save_environment_state_ref(
        &self,
        session_id: &SessionId,
        environment_state: EnvironmentStateRef,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let session = inner
            .sessions
            .get_mut(session_id)
            .ok_or_else(|| SessionStoreError::NotFound(session_id.as_str().to_string()))?;
        session.environment_state = Some(environment_state);
        session.updated_at = Utc::now();
        Ok(())
    }
}
