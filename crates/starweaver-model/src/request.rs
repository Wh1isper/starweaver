//! Request preparation snapshots and profile-driven normalization.

mod instructions;
mod normalization;
mod prepare;
mod types;

pub use instructions::{
    context_origin_metadata, InstructionPart, PreparedInstruction,
    CONTEXT_ORIGIN_ENVIRONMENT_CONTEXT, CONTEXT_ORIGIN_HANDOFF, CONTEXT_ORIGIN_METADATA,
    CONTEXT_ORIGIN_RUNTIME_CONTEXT, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, CONTEXT_TYPE_METADATA,
    INSTRUCTION_DYNAMIC_METADATA, INSTRUCTION_ORIGIN_AGENT, INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION,
    INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_TOOLSET,
};
pub(crate) use instructions::{current_instruction_request_index, is_dynamic_instruction_metadata};
pub use normalization::prepare_messages;
pub use prepare::{attach_prepared_instructions, prepare_model_request};
pub use types::{OutputMode, PreparedModelRequest};
