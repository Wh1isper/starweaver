//! Shell review public types, configuration, prompt rendering, and history.

mod handle;
mod policy;
mod request;

pub use handle::{attach_shell_review, attach_shell_review_handle, ShellReviewHandle};
pub use policy::{ShellReviewAction, ShellReviewConfig, ShellReviewDecision, ShellReviewRiskLevel};
pub(super) use request::ShellReviewFingerprint;
pub use request::{ShellReviewContextSnapshot, ShellReviewPreviousDecision, ShellReviewRequest};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Stored shell review result for short-term reviewer context.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewRecord {
    /// Original request.
    pub request: ShellReviewRequest,
    /// Review decision.
    pub decision: ShellReviewDecision,
    /// Runtime tool call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Whether execution was approved.
    #[serde(default)]
    pub approved: bool,
    /// Record creation timestamp.
    pub created_at: DateTime<Utc>,
}

impl ShellReviewRecord {
    pub(crate) fn pending(
        request: ShellReviewRequest,
        decision: ShellReviewDecision,
        tool_call_id: Option<String>,
    ) -> Self {
        Self {
            request,
            decision,
            tool_call_id,
            approved: false,
            created_at: Utc::now(),
        }
    }
}
