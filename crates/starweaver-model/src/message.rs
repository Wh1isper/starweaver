//! Canonical message history and request/response parts.

use serde_json::{Map, Value};

/// Serializable metadata object used by model messages and parts.
pub type Metadata = Map<String, Value>;

mod history;
mod provider;
mod request_parts;
mod response_parts;
mod tool;

pub use history::{ModelMessage, ModelRequest, ModelResponse};
pub use provider::{FinishReason, ProviderInfo, ProviderPartInfo};
pub use request_parts::{CachePointTtl, ContentPart, ModelRequestPart};
pub use response_parts::ModelResponsePart;
pub use tool::{
    TOOL_RETURN_APPROVAL_ARGUMENTS_METADATA_KEY, ToolArguments, ToolCallPart, ToolReturnPart,
};
