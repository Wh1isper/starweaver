//! Runtime helper error mapping.

use crate::{
    agent::{Agent, AgentError},
    capability::CapabilityError,
    instructions::DynamicInstructionError,
    output::OutputValidationError,
};

impl Agent {
    pub(in crate::agent) fn dynamic_instruction_error(
        error: DynamicInstructionError,
    ) -> AgentError {
        match error {
            DynamicInstructionError::Failed(message) => AgentError::DynamicInstruction(message),
        }
    }

    pub(in crate::agent) fn output_validation_error(
        error: OutputValidationError,
    ) -> CapabilityError {
        match error {
            OutputValidationError::InvalidJson(message)
            | OutputValidationError::Schema(message)
            | OutputValidationError::Retry(message) => CapabilityError::ModelRetry(message),
            OutputValidationError::Failed(message) => CapabilityError::Failed(message),
        }
    }

    pub(in crate::agent) fn capability_error(error: CapabilityError) -> AgentError {
        match error {
            CapabilityError::ModelRetry(message) => AgentError::Capability(format!(
                "unexpected retry outside output validation: {message}"
            )),
            CapabilityError::SkipModelRequest(_) => {
                AgentError::Capability("unexpected skip model request".to_string())
            }
            CapabilityError::Failed(message) => AgentError::Capability(message),
            CapabilityError::Cancelled { reason } => AgentError::Cancelled { reason },
        }
    }
}
