use std::sync::Arc;

use serde::Serialize;
use starweaver_tools::{
    DynTool, DynToolset, StaticToolset, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool_with_metadata, tool_execution_error, tool_metadata};

mod args;
mod download;
mod http;
mod media;
mod web;

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

const SEARCH_GUIDELINES: &str = r"<search-tool>
Search the web for information using search APIs.

<best-practices>
- Keep queries concise and specific for better results.
- Refine broad queries by reducing keywords if results are not ideal.
</best-practices>
</search-tool>";

const FETCH_GUIDELINES: &str = r"<fetch-tool>
Read web files or check resource availability via HTTP.

<best-practices>
- Use head_only=true to check existence without downloading content.
- For large files, content is truncated; use `download` instead.
- For PDF files, download first and then use a document conversion workflow when available.
- Returns content_type, content_length, and status_code for HEAD requests.
</best-practices>
</fetch-tool>";

const SCRAPE_GUIDELINES: &str = r"<scrape-tool>
Convert websites to Markdown format for content analysis.

<best-practices>
- Always use full URLs: https://example.com (not example.com).
- Content over the configured size limit is auto-truncated; use `download` to save full source.
- Uses the configured host scrape adapter when available, otherwise falls back to direct HTTP text extraction.
</best-practices>
</scrape-tool>";

const DOWNLOAD_GUIDELINES: &str = r"<download-tool>
Download files from URLs and save to the active environment.

<best-practices>
- Downloads multiple URLs into save_dir.
- Files are saved with generated names; use `move` to rename if needed.
- For PDF content, download first and then use a document conversion workflow when available.
- For web page content, use `scrape` instead.
- For quick viewing without saving, use `fetch`.
</best-practices>
</download-tool>";

const LOAD_MEDIA_URL_GUIDELINES: &str = r"<load-media-url-tool>

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

</load-media-url-tool>";

/// Create host I/O tools for web access, downloads, and media understanding.
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
        ToolInstruction::new(
            "host_io",
            "These tools expose host-provided I/O capabilities such as web access, downloads, and media analysis.",
        ),
        ToolInstruction::new("search", SEARCH_GUIDELINES),
        ToolInstruction::new("fetch", FETCH_GUIDELINES),
        ToolInstruction::new("scrape", SCRAPE_GUIDELINES),
        ToolInstruction::new("download", DOWNLOAD_GUIDELINES),
        ToolInstruction::new("load_media_url", LOAD_MEDIA_URL_GUIDELINES),
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
            "read_image",
            "Read and analyze an image from a URL. Use when native vision is unavailable.",
            tool_metadata("host_io", false, false),
            read_image,
        ),
        static_tool_with_metadata(
            "read_video",
            "Read and analyze a video from a URL. Use when native video understanding is unavailable.",
            tool_metadata("host_io", false, false),
            read_video,
        ),
        static_tool_with_metadata(
            "read_audio",
            "Read and analyze audio from a URL. Use when native audio understanding is unavailable.",
            tool_metadata("host_io", false, false),
            read_audio,
        ),
        static_tool_with_metadata(
            "load_media_url",
            "Load multimedia content directly from HTTP/HTTPS URL when model capabilities support it.",
            tool_metadata("host_io", false, false),
            load_media_url,
        ),
    ]
}

fn json_result(value: impl Serialize, tool: &str) -> Result<ToolResult, ToolError> {
    serde_json::to_value(value)
        .map(ToolResult::new)
        .map_err(|error| tool_execution_error(tool, error))
}
