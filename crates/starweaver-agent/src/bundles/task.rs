//! Task management tool bundle.

mod args;
mod formatting;
mod instructions;
mod operations;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use starweaver_tools::{DynToolset, StaticToolset};

use super::helpers::{static_tool_with_metadata, tool_metadata};
use instructions::task_manager_instructions;
use operations::{task_create, task_get, task_list, task_update};

/// Create task operation tools.
#[must_use]
pub fn task_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("task")
            .with_id("task")
            .with_instruction(task_manager_instructions())
            .with_tools([
                static_tool_with_metadata(
                    "task_create",
                    "Create a new task. Task status defaults to pending.",
                    tool_metadata("task", true, false),
                    task_create,
                ),
                static_tool_with_metadata(
                    "task_get",
                    "Get task details by ID.",
                    tool_metadata("task", true, false),
                    task_get,
                ),
                static_tool_with_metadata(
                    "task_update",
                    "Update task status, content, or dependencies.",
                    tool_metadata("task", true, false),
                    task_update,
                ),
                static_tool_with_metadata(
                    "task_list",
                    "List all tasks and their status.",
                    tool_metadata("task", true, false),
                    task_list,
                ),
            ]),
    )
}
