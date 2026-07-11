use std::sync::Arc;

use starweaver_tools::{DynToolset, StaticToolset, ToolInstruction};

use super::helpers::{
    context_management_tool_metadata, static_sequential_tool_with_metadata,
    static_tool_with_metadata, tool_metadata,
};

mod args;
mod context;

use context::{note_get, note_set, summarize, thinking};

const SUMMARIZE_GUIDELINES: &str = r#"<summarize-guidelines>

<communication>
When summarizing, communicate naturally with the user:
- "The conversation is getting long. Let me summarize our progress and continue."
- "Before we switch to the new task, let me summarize what we've done so far."
- "Let me organize our progress, then we can move on to [next topic]."

Do NOT use technical jargon like "context reset", "context window", or "token limit" with the user.
</communication>

<when-to-summarize>
- System reminder indicates the conversation is getting large.
- Conversation has accumulated back-and-forth that is no longer relevant.
- About to begin multi-step work that benefits from a clean handoff.
- User asks to work on a different topic or explicitly asks to summarize and continue.
</when-to-summarize>

<before-summarizing>
1. Capture remaining work as tasks if applicable.
2. Organize notes before summarizing if note tools are available.
3. Identify key files the resumed agent may need to inspect on demand.
4. Note important decisions, architecture choices, and user preferences.
</before-summarizing>

<content-structure>
The `content` field should be concise but complete:

```
## User Intent
[What the user is trying to accomplish]

## Current State
[What has been done, current progress]

## Key Decisions
- [Decision 1]: [Rationale]

## Past Interactions
- [Concise log of key interactions that already occurred]

## Next Step
[Immediate action to take after summary]
```
</content-structure>

<files-to-inspect>
List only files that may need to be inspected immediately after summary. Their paths will be added to a reminder, but their contents will not be loaded into context. The resumed agent should inspect them on demand with filesystem tools. Avoid temporary files and files already described sufficiently in the summary.
</files-to-inspect>

</summarize-guidelines>"#;

const NOTE_GUIDELINES: &str = r#"<note-guidelines>

<when-to-use>
- User states a preference that should be remembered for this session.
- Important facts or decisions that you need to recall later.
- Context that would be lost after summarize/compact.
- Intermediate results worth preserving.
</when-to-use>

<best-practices>
- Use descriptive, stable keys such as "user-language" or "project-framework".
- Keep values concise and delete entries when they are stale.
- Use `note_get` when runtime context lists a relevant note key and the value is needed.
- Store large data in files and keep only the file path or index in notes.
</best-practices>

</note-guidelines>"#;

const THINKING_GUIDELINES: &str = r"<thinking-guidelines>

<when-to-use>
Use `thinking` for complex reasoning or to cache intermediate thoughts.
</when-to-use>

<appropriate-scenarios>
- Complex multi-step reasoning that benefits from explicit thinking.
- Caching intermediate analysis or observations for later reference.
- Breaking down problems before taking action.
</appropriate-scenarios>

<inappropriate-scenarios>
- Task planning and management (use task tools instead).
- Simple straightforward operations.
</inappropriate-scenarios>

<language>
Use the user's language when writing thoughts.
</language>

</thinking-guidelines>";

/// Create context tools for handoff, notes, and explicit thinking.
#[must_use]
pub fn context_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("context")
            .with_id("context")
            .with_instructions(context_tool_instructions())
            .with_tools(context_tool_definitions()),
    )
}

fn context_tool_instructions() -> Vec<ToolInstruction> {
    vec![
        ToolInstruction::new("summarize", SUMMARIZE_GUIDELINES),
        ToolInstruction::new("note", NOTE_GUIDELINES),
        ToolInstruction::new("thinking", THINKING_GUIDELINES),
    ]
}

fn context_tool_definitions() -> Vec<starweaver_tools::DynTool> {
    vec![
        static_sequential_tool_with_metadata(
            "summarize",
            "Summarize current work and clear context to start fresh.",
            context_management_tool_metadata("context", true, false),
            summarize,
        ),
        static_tool_with_metadata(
            "note",
            "Create, update, or delete a note entry.",
            tool_metadata("context", true, false),
            note_set,
        ),
        static_tool_with_metadata(
            "note_get",
            "Read note entries by key, or list all note entries.",
            tool_metadata("context", true, false),
            note_get,
        ),
        static_tool_with_metadata(
            "thinking",
            "Think about something without obtaining new information or making changes.",
            tool_metadata("context", true, false),
            thinking,
        ),
    ]
}
