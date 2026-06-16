use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::message::{Metadata, ModelMessage, ModelRequest, ModelRequestPart};

/// Metadata key indicating whether an instruction is dynamic for prompt-cache placement.
pub const INSTRUCTION_DYNAMIC_METADATA: &str = "starweaver_instruction_dynamic";
/// Metadata key describing the instruction source/origin.
pub const INSTRUCTION_ORIGIN_METADATA: &str = "starweaver_instruction_origin";
/// Metadata key describing SDK/context prompt source/origin.
pub const CONTEXT_ORIGIN_METADATA: &str = "starweaver_context_origin";
/// Metadata key describing SDK/context prompt type.
pub const CONTEXT_TYPE_METADATA: &str = "starweaver_context_type";

/// Static agent instruction origin.
pub const INSTRUCTION_ORIGIN_AGENT: &str = "agent_instruction";
/// Toolset instruction origin.
pub const INSTRUCTION_ORIGIN_TOOLSET: &str = "toolset";
/// Dynamic agent instruction origin.
pub const INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION: &str = "dynamic_instruction";

/// Runtime context origin.
pub const CONTEXT_ORIGIN_RUNTIME_CONTEXT: &str = "runtime_context";
/// Environment context origin.
pub const CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT: &str = "environment_context";
/// SDK handoff context origin.
pub const CONTEXT_ORIGIN_HANDOFF: &str = "handoff";
/// Tool-return media control prompt origin.
pub const CONTEXT_ORIGIN_TOOL_RETURN_MEDIA: &str = "tool_return_media";

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn is_dynamic_instruction_metadata(metadata: &Metadata) -> bool {
    metadata
        .get(INSTRUCTION_DYNAMIC_METADATA)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Return SDK/context prompt origin metadata from a request part.
#[must_use]
pub fn context_origin_metadata(metadata: &Metadata) -> Option<&str> {
    metadata
        .get(CONTEXT_ORIGIN_METADATA)
        .and_then(Value::as_str)
}

/// Return the request whose instruction material should be applied to the current model call.
///
/// This mirrors Starweaver's separation between durable message history and current
/// request `instruction_parts`: instruction material in older requests remains part of
/// the session history but is not re-applied as current system/developer instructions.
/// If the newest request only carries tool returns/retry prompts and has no instruction
/// material, fall back to the preceding request's instructions for direct/replay callers.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn current_instruction_request_index(messages: &[ModelMessage]) -> Option<usize> {
    let latest = messages
        .iter()
        .rposition(|message| matches!(message, ModelMessage::Request(_)))?;
    let ModelMessage::Request(request) = &messages[latest] else {
        unreachable!("latest request index points at a request")
    };
    if request_has_instruction_material(request) {
        return Some(latest);
    }
    if request_is_control_only(request) {
        return messages[..latest].iter().rposition(|message| {
            matches!(message, ModelMessage::Request(request) if request_has_instruction_material(request))
        });
    }
    None
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn request_has_instruction_material(request: &ModelRequest) -> bool {
    request
        .instructions
        .as_deref()
        .is_some_and(|instructions| !instructions.trim().is_empty())
        || request
            .parts
            .iter()
            .any(|part| matches!(part, ModelRequestPart::Instruction { .. }))
}

fn request_is_control_only(request: &ModelRequest) -> bool {
    !request.parts.is_empty()
        && request.parts.iter().all(|part| match part {
            ModelRequestPart::ToolReturn(_) | ModelRequestPart::RetryPrompt { .. } => true,
            ModelRequestPart::UserPrompt { metadata, .. } => context_origin_metadata(metadata)
                .is_some_and(|origin| {
                    matches!(
                        origin,
                        CONTEXT_ORIGIN_TOOL_RETURN_MEDIA
                            | CONTEXT_ORIGIN_RUNTIME_CONTEXT
                            | CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT
                            | CONTEXT_ORIGIN_HANDOFF
                    )
                }),
            ModelRequestPart::SystemPrompt { .. } | ModelRequestPart::Instruction { .. } => false,
        })
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

    /// Attach instruction origin metadata.
    #[must_use]
    pub fn with_origin(mut self, origin: impl Into<String>) -> Self {
        self.metadata.insert(
            INSTRUCTION_ORIGIN_METADATA.to_string(),
            serde_json::json!(origin.into()),
        );
        self
    }

    /// Attach instruction dynamic metadata and update the typed dynamic flag.
    #[must_use]
    pub fn with_dynamic(mut self, dynamic: bool) -> Self {
        self.dynamic = dynamic;
        self.metadata.insert(
            INSTRUCTION_DYNAMIC_METADATA.to_string(),
            serde_json::json!(dynamic),
        );
        self
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
            INSTRUCTION_DYNAMIC_METADATA.to_string(),
            serde_json::json!(self.dynamic),
        );
        ModelRequestPart::Instruction {
            text: self.text.clone(),
            metadata,
        }
    }
}

/// Alias for structured instruction parts.
pub type InstructionPart = PreparedInstruction;
