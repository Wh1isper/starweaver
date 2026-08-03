//! Agent runtime helper method modules.

mod capability_hooks;
mod checkpoint;
mod compact_context;
mod errors;
mod history_sanitize;
mod output_validation;
mod prepare_tools_safety;
mod previous_response;
mod request_building;
mod request_parts;
mod steering;
mod tool_media;
mod trace_events;
mod usage_limits;

pub(in crate::agent) use self::{
    prepare_tools_safety::validate_prepared_tools,
    request_parts::request_instruction_insert_index,
    tool_media::{
        GEOMETRY_BOUND_MEDIA_KEY, TOOL_RETURN_CONTENT_PARTS_KEY, TOOL_RETURN_MEDIA_PROMPT_KEY,
        tool_return_media_prompt,
    },
};
