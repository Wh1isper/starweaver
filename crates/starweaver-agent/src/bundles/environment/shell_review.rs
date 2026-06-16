//! Shell command safety review helpers.

mod execution;
mod parsing;
#[cfg(test)]
mod tests;
mod types;

pub use execution::review_shell_command_or_block;
pub use types::{
    attach_shell_review, attach_shell_review_handle, ShellReviewAction, ShellReviewConfig,
    ShellReviewContextSnapshot, ShellReviewDecision, ShellReviewHandle,
    ShellReviewPreviousDecision, ShellReviewRecord, ShellReviewRequest, ShellReviewRiskLevel,
};

/// Default shell-command review prompt.
pub const DEFAULT_SHELL_REVIEW_PROMPT: &str = "You review shell commands before execution.\n\nReturn a risk_level and concise reason. Commands below the configured risk threshold\nexecute directly. Commands at or above the threshold enter the configured approval or deny policy.\n\nRisk heuristics:\n- low: read-only inspection or local developer verification such as tests, lint, type checks,\n  imports, and printing.\n- medium: bounded workspace-local state changes such as file writes/deletes, generated\n  artifact or cache cleanup/generation, chmod/chown, package changes, and local servers.\n- high: untrusted remote code execution, broad destructive workspace changes, writes outside\n  the workspace, sensitive file reads, sudo usage, or system-level package/service changes.\n- extra_high: confirmed credential exfiltration, destructive home/root deletion, explicit\n  privilege escalation, malware-like behavior, or broad external data upload.\n\nReserve extra_high for visible catastrophic or hostile intent. Remote script execution is high by default.\nClassify Python and uv commands by the script's visible effect; `uv run python` alone is a runner. Python compileall writes __pycache__/bytecode and is medium risk. Explicit cache/bytecode generation and outbound network access need approval.\nUse workspace context for path scope. Source/package-tree wildcard deletion is broad destructive workspace change.\nUse previous shell reviews as consistency hints. When an identical or equivalent command was previously approved and the current visible effect has not expanded, lower the risk by at least one level. Repeated approved commands that remain bounded and workspace-local can be low risk.\nWhen a command combines safe and risky operations, classify by the riskiest operation after applying relevant previous approval context.\nReturn a concise reason.\n";
