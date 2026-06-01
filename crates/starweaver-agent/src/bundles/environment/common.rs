use serde_json::Value;
use starweaver_tools::{ToolError, ToolResult};

use crate::bundles::helpers::tool_execution_error;

pub(super) fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
}

pub(super) fn non_negative_limit(
    tool: &str,
    field: &str,
    value: isize,
) -> Result<usize, ToolError> {
    if value < 0 {
        return Err(tool_execution_error(
            tool,
            format!("{field} must be greater than or equal to 0"),
        ));
    }
    usize::try_from(value).map_err(|error| tool_execution_error(tool, error))
}

pub(super) fn limit_or_unlimited(
    tool: &str,
    field: &str,
    value: isize,
) -> Result<usize, ToolError> {
    if value < -1 {
        return Err(tool_execution_error(
            tool,
            format!("{field} must be greater than or equal to -1"),
        ));
    }
    Ok(if value == -1 {
        0
    } else {
        usize::try_from(value).map_err(|error| tool_execution_error(tool, error))?
    })
}
