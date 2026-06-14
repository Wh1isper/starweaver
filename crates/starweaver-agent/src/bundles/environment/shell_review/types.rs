//! Shell review public types, configuration, prompt rendering, and history.

mod handle;
mod policy;
mod record;
mod request;

pub use handle::{attach_shell_review, attach_shell_review_handle, ShellReviewHandle};
pub use policy::{ShellReviewAction, ShellReviewConfig, ShellReviewDecision, ShellReviewRiskLevel};
pub use record::ShellReviewRecord;
pub(super) use request::ShellReviewFingerprint;
pub use request::{ShellReviewContextSnapshot, ShellReviewPreviousDecision, ShellReviewRequest};
