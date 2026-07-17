//! Terminal UI rendering built from display messages.

mod markdown;
mod render;
mod shell;
mod snapshot;
mod state;
mod terminal;
mod timeline;

#[cfg(test)]
mod tests;

pub use shell::{TuiShellEvent, TuiShellRun, spawn_shell_run};
pub use snapshot::TuiSnapshot;
pub use state::{InteractiveTuiState, ModelChoice, SessionChoice};
pub use terminal::{InteractiveTui, InteractiveTuiEvent, TuiApprovalDecision};
