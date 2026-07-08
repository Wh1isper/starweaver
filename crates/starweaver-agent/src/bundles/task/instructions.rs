//! Task tool instructions.

use starweaver_tools::ToolInstruction;

#[allow(clippy::needless_raw_string_hashes)]
pub(super) fn task_manager_instructions() -> ToolInstruction {
    ToolInstruction::new(
        "task-manager",
        r#"<task-manager-guidelines>

<when-to-use-tasks>
Use task tools when:
- The request has multiple meaningful steps, files, modules, phases, or validation gates.
- Work can be split across dependencies or parallel tracks.
- Progress needs to survive context changes, long runs, interruptions, or delegated subagent work.

Do not create tasks for a single direct action, quick lookup, tiny edit, or answer that can be completed immediately. If there is only one real task, it is usually better to do it directly without `task_create`.
</when-to-use-tasks>

<task-granularity>
When you do create tasks:
- Write task content in the user's language.
- Make each task specific enough to execute and verify.
- Prefer concrete outcomes over process labels.
- Avoid vague umbrella tasks such as "finish the request", "handle the project", or "do the work".
- If a broad task is necessary, immediately decompose it into concrete tasks before starting.
- Do not split into busywork; each task should represent meaningful progress.
</task-granularity>

<workflow>
Status: pending -> in_progress -> completed.
- Set in_progress only when actively working on that task.
- Set completed immediately after finishing and verifying the task.
- Completed tasks automatically unblock dependents.
- Do not update status for every tiny internal action; use task updates for meaningful state changes.
</workflow>

<dependencies>
- Set up dependencies early when planning.
- Add dependencies only for true blockers, not every sequential preference.
</dependencies>

<delegation-coordination>
Task planning can help identify independent work for subagents.
- Create tasks for parallel tracks before delegating when it improves coordination.
- Follow the delegate tool's own execution model; task instructions do not define whether delegate is blocking or asynchronous.
- When subagent results arrive, update the relevant task with outcome, changed files, tests, and risks.
</delegation-coordination>

</task-manager-guidelines>"#,
    )
}
