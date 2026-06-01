//! Resume snapshots assembled from durable session state.

use serde::{Deserialize, Serialize};
use starweaver_context::ResumableState;
use starweaver_runtime::{AgentCheckpoint, AgentStreamRecord};

use crate::{
    approval::{ApprovalRecord, DeferredToolRecord},
    records::{EnvironmentStateRef, RunRecord, SessionRecord, StreamCursorRef},
};

/// Resume package loaded from a durable session store.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionResumeSnapshot {
    /// Session record.
    pub session: SessionRecord,
    /// Run record.
    pub run: RunRecord,
    /// Exported context state.
    pub state: ResumableState,
    /// Latest environment state reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_state: Option<EnvironmentStateRef>,
    /// Latest checkpoint for the requested run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<AgentCheckpoint>,
    /// Replayable stream records after the checkpoint cursor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_records: Vec<AgentStreamRecord>,
    /// Pending approval records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approvals: Vec<ApprovalRecord>,
    /// Pending deferred tool records.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_tools: Vec<DeferredToolRecord>,
    /// Latest stream cursor references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stream_cursors: Vec<StreamCursorRef>,
}
