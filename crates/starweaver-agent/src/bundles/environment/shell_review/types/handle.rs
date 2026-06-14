use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use starweaver_context::AgentContext;

use super::{ShellReviewConfig, ShellReviewFingerprint, ShellReviewRecord};

const SHELL_REVIEW_HISTORY_LIMIT: usize = 10;

/// `AgentContext` dependency carrying shell review config and short-term history.
#[derive(Clone, Debug)]
pub struct ShellReviewHandle {
    config: ShellReviewConfig,
    records: Arc<Mutex<VecDeque<ShellReviewRecord>>>,
}

impl ShellReviewHandle {
    /// Create a shell review handle.
    #[must_use]
    pub fn new(config: ShellReviewConfig) -> Self {
        Self {
            config,
            records: Arc::new(Mutex::new(VecDeque::with_capacity(
                SHELL_REVIEW_HISTORY_LIMIT,
            ))),
        }
    }

    /// Return the review configuration.
    #[must_use]
    pub const fn config(&self) -> &ShellReviewConfig {
        &self.config
    }

    /// Return a snapshot of previous records.
    #[must_use]
    pub fn records(&self) -> Vec<ShellReviewRecord> {
        self.records
            .lock()
            .map_or_else(|_| Vec::new(), |records| records.iter().cloned().collect())
    }

    pub(crate) fn push_record(&self, record: ShellReviewRecord) {
        if let Ok(mut records) = self.records.lock() {
            if records.len() >= SHELL_REVIEW_HISTORY_LIMIT {
                records.pop_front();
            }
            records.push_back(record);
        }
    }

    pub(crate) fn update_last_matching_approval(
        &self,
        tool_call_id: Option<&str>,
        fingerprint: &ShellReviewFingerprint,
    ) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        if let Some(tool_call_id) = tool_call_id {
            if let Some(record) = records
                .iter_mut()
                .rev()
                .find(|record| record.tool_call_id.as_deref() == Some(tool_call_id))
            {
                record.approved = true;
                return;
            }
        }
        if let Some(record) = records
            .iter_mut()
            .rev()
            .find(|record| record.request.command_fingerprint() == *fingerprint)
        {
            record.approved = true;
        }
    }
}

/// Attach shell command review to an `AgentContext`.
pub fn attach_shell_review(context: &mut AgentContext, config: ShellReviewConfig) {
    context.dependencies.insert(ShellReviewHandle::new(config));
}

/// Attach a shared shell review handle to an `AgentContext`.
pub fn attach_shell_review_handle(context: &mut AgentContext, handle: ShellReviewHandle) {
    context.dependencies.insert(handle);
}
