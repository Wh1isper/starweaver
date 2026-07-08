use std::sync::Arc;

use serde::Serialize;
use starweaver_tools::{
    DynTool, DynToolset, StaticToolset, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{
    static_tool_with_metadata, tool_execution_error, tool_feedback, tool_metadata,
};

mod args;
mod download;
mod http;
mod media;
mod web;

use download::download;
use media::read_media;
use web::{fetch, scrape, search};

pub use media::{
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse,
};
pub use web::{
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem,
};

const SEARCH_GUIDELINES: &str = r"<search-guidelines>
<best-practices>
- Prefer primary or authoritative sources when the query has factual, legal, financial, medical, or product implications.
- Start with specific queries and refine with entity names, dates, versions, or source types when results are noisy.
- Cross-check important claims across independent sources before relying on them.
</best-practices>
</search-guidelines>";

const FETCH_GUIDELINES: &str = r"<fetch-guidelines>
<best-practices>
- Use head_only=true to check existence without downloading content.
- For large files, content is truncated; use `download` instead.
- For PDF files, download first and then use a document conversion workflow when available.
</best-practices>
</fetch-guidelines>";

const SCRAPE_GUIDELINES: &str = r"<scrape-guidelines>
<best-practices>
- Always use full URLs: https://example.com (not example.com).
- Content over the configured size limit is auto-truncated; use `download` to save full source.
</best-practices>
</scrape-guidelines>";

const DOWNLOAD_GUIDELINES: &str = r"<download-guidelines>
<best-practices>
- Files are saved with generated names; use `move` to rename if needed.
- For PDF content, download first and then use a document conversion workflow when available.
- For web page content, use `scrape` instead.
- For quick viewing without saving, use `fetch`.
</best-practices>
</download-guidelines>";

const READ_MEDIA_GUIDELINES: &str = r"<read-media-guidelines>
<best-practices>
- Use this when the user gives a direct media URL and asks about the media content.
- Pass focused analysis instructions when the user wants a specific detail, timestamp, transcription, or comparison.
- YouTube URLs are passed directly only when the active model advertises YouTube URL support; otherwise the configured fallback media understanding adapter is used.
- Large files, unsupported formats, and documents should be downloaded first, then inspected with `view` or a document conversion workflow.
</best-practices>
</read-media-guidelines>";

/// Create host I/O tools for web access and downloads.
#[must_use]
pub fn host_io_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("host_io")
            .with_id("host_io")
            .with_instructions(host_io_tool_instructions())
            .with_tools(host_io_tool_definitions()),
    )
}

fn host_io_tool_instructions() -> Vec<ToolInstruction> {
    vec![
        ToolInstruction::new("search", SEARCH_GUIDELINES),
        ToolInstruction::new("fetch", FETCH_GUIDELINES),
        ToolInstruction::new("scrape", SCRAPE_GUIDELINES),
        ToolInstruction::new("download", DOWNLOAD_GUIDELINES),
        ToolInstruction::new("read_media", READ_MEDIA_GUIDELINES),
    ]
}

fn host_io_tool_definitions() -> Vec<DynTool> {
    vec![
        static_tool_with_metadata(
            "search",
            "Search the web for information using search APIs.",
            tool_metadata("host_io", false, false),
            search,
        ),
        static_tool_with_metadata(
            "fetch",
            "Read web files or check resource availability via HTTP.",
            tool_metadata("host_io", false, false),
            fetch,
        ),
        static_tool_with_metadata(
            "scrape",
            "Convert websites to Markdown format for content analysis.",
            tool_metadata("host_io", false, false),
            scrape,
        ),
        static_tool_with_metadata(
            "download",
            "Download files from URLs and save to the active environment.",
            tool_metadata("host_io", false, false),
            download,
        ),
        static_tool_with_metadata(
            "read_media",
            "Read an HTTP/HTTPS image, video, audio, or supported YouTube URL as model-consumable media.",
            tool_metadata("host_io", false, false),
            read_media,
        ),
    ]
}

fn json_result(value: impl Serialize, tool: &str) -> Result<ToolResult, ToolError> {
    let value = serde_json::to_value(value).map_err(|error| tool_execution_error(tool, error))?;
    if value.get("success").and_then(serde_json::Value::as_bool) == Some(false) {
        return Err(unsuccessful_tool_result(tool, &value));
    }
    Ok(ToolResult::new(value))
}

fn unsuccessful_tool_result(tool: &str, value: &serde_json::Value) -> ToolError {
    let message = unsuccessful_result_message(tool, value);
    tool_feedback(tool, message)
}

fn unsuccessful_result_message(tool: &str, value: &serde_json::Value) -> String {
    if let Some(message) = value.get("message").and_then(serde_json::Value::as_str) {
        return message.to_string();
    }
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return error.to_string();
    }
    if let Some(errors) = value.get("errors").and_then(serde_json::Value::as_array) {
        let joined = errors
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>()
            .join("; ");
        if !joined.is_empty() {
            return joined;
        }
    }
    format!("{tool} returned an unsuccessful response: {value}")
}
