//! Task management tool bundle.

mod args;
mod formatting;
mod instructions;
mod operations;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use starweaver_context::{AgentContext, CONTEXT_TASKS_CAPABILITY, ToolCapabilityGrant};
use starweaver_tools::{DynToolset, StaticToolset, ToolDependencyRequirements};

use super::helpers::{
    static_sequential_tool_with_metadata, static_tool_with_metadata,
    tool_metadata_with_dependencies,
};
use instructions::task_manager_instructions;
use operations::{task_create, task_get, task_list, task_update};

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn attach_task_tool_grants(context: &mut AgentContext) {
    for tool_name in ["task_create", "task_get", "task_update", "task_list"] {
        context.grant_tool_capabilities(
            tool_name,
            ToolCapabilityGrant::new().with_context_capabilities([CONTEXT_TASKS_CAPABILITY]),
        );
    }
}

/// Create task operation tools.
#[must_use]
pub fn task_tools() -> DynToolset {
    let metadata = tool_metadata_with_dependencies(
        "task",
        true,
        false,
        &ToolDependencyRequirements::strict(
            Vec::<String>::new(),
            [CONTEXT_TASKS_CAPABILITY],
            false,
        ),
    );
    Arc::new(
        StaticToolset::new("task")
            .with_id("task")
            .with_instruction(task_manager_instructions())
            .with_tools([
                static_sequential_tool_with_metadata(
                    "task_create",
                    "Create a new task. Task status defaults to pending.",
                    metadata.clone(),
                    task_create,
                ),
                static_tool_with_metadata(
                    "task_get",
                    "Get task details by ID.",
                    metadata.clone(),
                    task_get,
                ),
                static_sequential_tool_with_metadata(
                    "task_update",
                    "Update task status, content, or dependencies.",
                    metadata.clone(),
                    task_update,
                ),
                static_tool_with_metadata(
                    "task_list",
                    "List all tasks and their status.",
                    metadata,
                    task_list,
                ),
            ]),
    )
}
