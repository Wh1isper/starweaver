use starweaver_tools::ToolError;

use crate::bundles::helpers::{tool_execution_error, tool_invalid_arguments};

pub(super) fn non_negative_limit(
    tool: &str,
    field: &str,
    value: isize,
) -> Result<usize, ToolError> {
    if value < 0 {
        return Err(tool_invalid_arguments(
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
        return Err(tool_invalid_arguments(
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
