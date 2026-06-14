//! Task tool instructions.

use starweaver_tools::ToolInstruction;

#[allow(clippy::needless_raw_string_hashes)]
pub(super) fn task_manager_instructions() -> ToolInstruction {
    ToolInstruction::new(
        "task-manager",
        r#"<task-manager-guidelines>

<overview>
Task management tools track multi-step work with dependencies. Use them for complex projects, breaking down work, and tracking progress.
</overview>

<tools>
- `task_create`: Create a new task (defaults to pending).
- `task_get`: Get task details by ID.
- `task_list`: List all tasks with status overview.
- `task_update`: Update task status, content, or dependencies.
</tools>

<workflow>
Status: pending -> in_progress -> completed.
- Set in_progress when starting work.
- Set completed immediately after finishing.
- Completed tasks automatically unblock dependents.
</workflow>

<dependencies>
- add_blocked_by: tasks that must complete before this one.
- add_blocks: tasks this one will block.
- Set up dependencies early when planning.
</dependencies>

<delegate-with-subagents>
Delegate calls are blocking -- the agent waits until the subagent finishes. Multiple delegate calls in the same response run concurrently.

1. Create tasks and identify which can run in parallel.
2. Call multiple delegates in a single response to run them concurrently.
3. When subagents return, update task status accordingly.

Sequential delegate calls across turns run serially.
</delegate-with-subagents>

</task-manager-guidelines>"#,
    )
}
