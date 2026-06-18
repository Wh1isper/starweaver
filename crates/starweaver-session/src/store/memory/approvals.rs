use starweaver_core::{RunId, SessionId};

use crate::{
    approval::{ApprovalRecord, DeferredToolRecord},
    error::{SessionStoreError, SessionStoreResult},
};

use super::{run_key, run_key_label, store_failed, InMemorySessionStore};

impl InMemorySessionStore {
    pub(super) fn append_approval_record(
        &self,
        approval: ApprovalRecord,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(&approval.session_id, &approval.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                &approval.session_id,
                &approval.run_id,
            )));
        }
        let approvals = inner.approvals.entry(key).or_default();
        approvals.retain(|existing| existing.approval_id != approval.approval_id);
        approvals.push(approval);
        Ok(())
    }

    pub(super) fn load_approval_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<ApprovalRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .approvals
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }

    pub(super) fn append_deferred_tool_record(
        &self,
        record: DeferredToolRecord,
    ) -> SessionStoreResult<()> {
        let mut inner = self.inner.lock().map_err(store_failed)?;
        let key = run_key(&record.session_id, &record.run_id);
        if !inner.runs.contains_key(&key) {
            return Err(SessionStoreError::NotFound(run_key_label(
                &record.session_id,
                &record.run_id,
            )));
        }
        let deferred_tools = inner.deferred_tools.entry(key).or_default();
        deferred_tools.retain(|existing| existing.deferred_id != record.deferred_id);
        deferred_tools.push(record);
        Ok(())
    }

    pub(super) fn load_deferred_tool_records(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> SessionStoreResult<Vec<DeferredToolRecord>> {
        let inner = self.inner.lock().map_err(store_failed)?;
        Ok(inner
            .deferred_tools
            .get(&run_key(session_id, run_id))
            .cloned()
            .unwrap_or_default())
    }
}
