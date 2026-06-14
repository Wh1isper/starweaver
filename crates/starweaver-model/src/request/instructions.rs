use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::message::ModelRequestPart;

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

/// Prepared instruction fragment attached to request parameters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PreparedInstruction {
    /// Instruction text.
    pub text: String,
    /// Whether this instruction came from a dynamic source.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dynamic: bool,
    /// Instruction metadata.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub metadata: Map<String, Value>,
}

impl PreparedInstruction {
    /// Create a static instruction.
    #[must_use]
    pub fn static_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            dynamic: false,
            metadata: Map::new(),
        }
    }

    /// Create a dynamic instruction.
    #[must_use]
    pub fn dynamic_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            dynamic: true,
            metadata: Map::new(),
        }
    }

    /// Sort instruction parts with static instructions before dynamic instructions.
    #[must_use]
    pub fn sorted(instructions: &[Self]) -> Vec<Self> {
        let mut sorted = instructions.to_vec();
        sorted.sort_by_key(|instruction| instruction.dynamic);
        sorted
    }

    pub(super) fn to_request_part(&self) -> ModelRequestPart {
        let mut metadata = self.metadata.clone();
        metadata.insert(
            "starweaver_instruction_dynamic".to_string(),
            serde_json::json!(self.dynamic),
        );
        ModelRequestPart::Instruction {
            text: self.text.clone(),
            metadata,
        }
    }
}

/// Pydantic AI-compatible alias for structured instruction parts.
pub type InstructionPart = PreparedInstruction;
