//! Tool instruction blocks.

use serde::{Deserialize, Serialize};

/// Toolset or tool instruction block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolInstruction {
    /// Instruction group used for deduplication.
    pub group: String,
    /// Instruction content.
    pub content: String,
}

impl ToolInstruction {
    /// Create an instruction block.
    #[must_use]
    pub fn new(group: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            content: content.into(),
        }
    }
}
