pub(super) const CACHE_FRIENDLY_COMPACT_INSTRUCTION: &str = "Generate a compact continuation summary for the conversation history.
Return only the summary text. Do not call tools.
Use this exact Markdown structure:

## Condensed conversation summary

### Analysis

[Brief analysis of the conversation and what matters for continuation.]

### Context

1. Primary Request and Intent:
   [User's explicit requests and intent]

2. Key Technical Concepts:
   - [Concepts, technologies, APIs, and architecture points]

3. Files and Code Sections:
   - [Files examined, edited, or created, with important details]

4. Problem Solving:
   [Problems solved and ongoing troubleshooting]

5. Pending Tasks:
   - [Explicit pending tasks]

6. Current Work:
   [Precise current work immediately before compaction]

7. Optional Next Step:
   [Direct next step aligned with the current work]

8. Past Interactions:
   - [Key interactions already completed, including actions and outcomes]

9. Skills Documentation:
   [If any /skills/ documentation was accessed, list the relevant skill files and remind the next agent to re-read them]

10. Auto-load Files:
   [List only file paths that should be auto-loaded when resuming]
";
pub(super) const CACHE_FRIENDLY_COMPACT_PROMPT: &str = "Compact the conversation history into the requested continuation summary format. Focus on details needed to continue the user's work accurately after older messages are removed. Return only the summary text.";
pub(super) const COMPACT_LIMIT_PROMPT: &str = "You have exceeded the maximum token limit for this conversation. Please provide a summary of the conversation so far and what you should work on next and I'll resume the conversation.";
pub(super) const MAX_COMPACT_INSTRUCTION_CHARS: usize = 12_000;
pub(super) const MAX_COMPACT_REPLAY_INSTRUCTION_CHARS: usize = 20_000;
pub(super) const PROJECT_GUIDANCE_TAG: &str = "project-guidance";
pub(super) const USER_RULES_TAG: &str = "user-rules";

pub(super) const COMPACT_KEEP_MESSAGES_METADATA: &str = "starweaver_compact_keep_messages";
pub(super) const COMPACT_DEPTH_METADATA: &str = "starweaver_compact_depth";
pub(super) const DEFAULT_AUTO_COMPACT_KEEP_MESSAGES: usize = 12;
