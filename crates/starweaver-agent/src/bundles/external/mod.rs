use std::sync::Arc;

use serde::Serialize;
use starweaver_tools::{DynToolset, StaticToolset, ToolError, ToolInstruction, ToolResult};

use super::helpers::{static_tool_with_metadata, tool_execution_error, tool_metadata};

mod args;
mod context;
mod download;
mod http;
mod media;
mod web;

use context::{note_get, note_set, summarize, thinking};
use download::download;
use media::{load_media_url, read_audio, read_image, read_video};
use web::{fetch, scrape, search};

pub use media::{
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse,
};
pub use web::{
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem,
};

/// Create host-operation tools for web, media, download, and context-management capabilities.
#[must_use]
#[allow(clippy::needless_raw_string_hashes, clippy::too_many_lines)]
pub fn host_operation_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("host_operations")
            .with_id("host_operations")
            .with_instruction(ToolInstruction::new(
                "host_operations",
                "These tools expose host-provided capabilities such as web access, media analysis, downloads, notes, thinking, and context handoff.",
            ))
            .with_instruction(ToolInstruction::new(
                "search",
                r#"<search-tool>
Search the web for information using search APIs.

<best-practices>
- Keep queries concise and specific for better results.
- Refine broad queries by reducing keywords if results are not ideal.
</best-practices>
</search-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "fetch",
                r#"<fetch-tool>
Read web files or check resource availability via HTTP.

<best-practices>
- Use head_only=true to check existence without downloading content.
- For large files, content is truncated; use `download` instead.
- For PDF files, download first and then use a document conversion workflow when available.
- Returns content_type, content_length, and status_code for HEAD requests.
</best-practices>
</fetch-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "scrape",
                r#"<scrape-tool>
Convert websites to Markdown format for content analysis.

<best-practices>
- Always use full URLs: https://example.com (not example.com).
- Content over the configured size limit is auto-truncated; use `download` to save full source.
- Uses the configured host scrape adapter when available, otherwise falls back to direct HTTP text extraction.
</best-practices>
</scrape-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "download",
                r#"<download-tool>
Download files from URLs and save to the active environment.

<best-practices>
- Downloads multiple URLs into save_dir.
- Files are saved with generated names; use `move` to rename if needed.
- For PDF content, download first and then use a document conversion workflow when available.
- For web page content, use `scrape` instead.
- For quick viewing without saving, use `fetch`.
</best-practices>
</download-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "load_media_url",
                r#"<load-media-url-tool>

<description>Load multimedia content (images, videos, audio, and supported documents) directly from HTTP/HTTPS URL for model analysis.</description>

<supported_urls>
- Images: `https://example.com/photo.jpg`, `https://example.com/image.png`
- Videos: `https://example.com/video.mp4`, `https://youtube.com/watch?v=xxx`
- Audio: `https://example.com/audio.mp3`, `https://example.com/recording.wav`
- Documents: `https://example.com/file.pdf` when document URL input is supported.
</supported_urls>

<activation>
Use this tool when the active model advertises native media/document URL capability. If the active model lacks the relevant capability, use `read_image`, `read_video`, or `read_audio` for media understanding, or `download` plus a document conversion workflow for documents.
</activation>

</load-media-url-tool>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "summarize",
                r#"<summarize-guidelines>

<overview>
The summarize tool captures current progress and starts fresh with a clean context. Use it for context management and switching focus between topics or tasks.
</overview>

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
3. Identify key files being actively edited or referenced.
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

</summarize-guidelines>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "note",
                r#"<note-guidelines>

<overview>
Note tools persist key-value information across conversation turns. Runtime context may include note keys; note values are read on demand with `note_get`.
</overview>

<tools>
- `note`: Create, update, or delete a note entry.
- `note_get`: Read a note entry by key, or omit key to read all note entries.
</tools>

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

</note-guidelines>"#,
            ))
            .with_instruction(ToolInstruction::new(
                "thinking",
                r#"<thinking-guidelines>

<when-to-use>
Use `thinking` for complex reasoning or to cache intermediate thoughts. The tool appends thoughts to the log without obtaining new information or making changes.
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

</thinking-guidelines>"#,
            ))
            .with_tools([
                static_tool_with_metadata("search", "Search the web for information using search APIs.", tool_metadata("host_operations", false, false), search),
                static_tool_with_metadata("fetch", "Read web files or check resource availability via HTTP.", tool_metadata("host_operations", false, false), fetch),
                static_tool_with_metadata("scrape", "Convert websites to Markdown format for content analysis.", tool_metadata("host_operations", false, false), scrape),
                static_tool_with_metadata("download", "Download files from URLs and save to the active environment.", tool_metadata("host_operations", false, false), download),
                static_tool_with_metadata("read_image", "Read and analyze an image from a URL. Use when native vision is unavailable.", tool_metadata("host_operations", false, false), read_image),
                static_tool_with_metadata("read_video", "Read and analyze a video from a URL. Use when native video understanding is unavailable.", tool_metadata("host_operations", false, false), read_video),
                static_tool_with_metadata("read_audio", "Read and analyze audio from a URL. Use when native audio understanding is unavailable.", tool_metadata("host_operations", false, false), read_audio),
                static_tool_with_metadata("load_media_url", "Load multimedia content directly from HTTP/HTTPS URL when model capabilities support it.", tool_metadata("host_operations", false, false), load_media_url),
                static_tool_with_metadata("summarize", "Summarize current work and clear context to start fresh.", tool_metadata("host_operations", true, false), summarize),
                static_tool_with_metadata("note", "Create, update, or delete a note entry.", tool_metadata("host_operations", true, false), note_set),
                static_tool_with_metadata("note_get", "Read note entries by key, or list all note entries.", tool_metadata("host_operations", true, false), note_get),
                static_tool_with_metadata("thinking", "Think about something without obtaining new information or making changes.", tool_metadata("host_operations", true, false), thinking),
            ]),
    )
}

fn json_result(value: impl Serialize, tool: &str) -> Result<ToolResult, ToolError> {
    serde_json::to_value(value)
        .map(ToolResult::new)
        .map_err(|error| tool_execution_error(tool, error))
}
