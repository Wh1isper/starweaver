use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ShellReviewDecision, ShellReviewRequest};

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
    pub(crate) fn new(
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
