//! Terminal UI rendering built from display messages.

mod markdown;
mod render;
mod snapshot;
mod state;
mod terminal;

#[cfg(test)]
mod tests;

pub use snapshot::TuiSnapshot;
pub use state::{GoalIterationOutcome, InteractiveTuiState, ModelChoice, SlashCommandDefinition};
pub use terminal::{InteractiveTui, InteractiveTuiEvent};
