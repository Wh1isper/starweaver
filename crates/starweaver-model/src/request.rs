//! Request preparation snapshots and profile-driven normalization.

mod instructions;
mod normalization;
mod prepare;
mod types;

pub(crate) use instructions::{current_instruction_request_index, is_dynamic_instruction_metadata};
pub use instructions::{
    InstructionPart, PreparedInstruction, INSTRUCTION_DYNAMIC_METADATA,
    INSTRUCTION_ORIGIN_DYNAMIC_INSTRUCTION, INSTRUCTION_ORIGIN_ENVIRONMENT_CONTEXT,
    INSTRUCTION_ORIGIN_HANDOFF, INSTRUCTION_ORIGIN_METADATA, INSTRUCTION_ORIGIN_RUNTIME_CONTEXT,
    INSTRUCTION_ORIGIN_TOOLSET,
};
pub use normalization::prepare_messages;
pub use prepare::{attach_prepared_instructions, prepare_model_request};
pub use types::{OutputMode, PreparedModelRequest};
