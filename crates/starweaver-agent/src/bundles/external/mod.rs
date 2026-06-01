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
pub fn host_operation_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("host_operations")
            .with_id("host_operations")
            .with_instruction(ToolInstruction::new(
                "host_operations",
                "These tools expose host-provided capabilities such as web access, media analysis, downloads, notes, thinking, and context handoff.",
            ))
            .with_tools([
                static_tool_with_metadata("search", "Search the web for information using search APIs.", tool_metadata("host_operations", false, false), search),
                static_tool_with_metadata("fetch", "Read web files or check resource availability via HTTP.", tool_metadata("host_operations", false, false), fetch),
                static_tool_with_metadata("scrape", "Convert websites to Markdown format for content analysis.", tool_metadata("host_operations", false, false), scrape),
                static_tool_with_metadata("download", "Download text resources from URLs and save them to the active environment.", tool_metadata("host_operations", false, false), download),
                static_tool_with_metadata("read_image", "Read and analyze an image from a URL through a configured fallback model.", tool_metadata("host_operations", false, false), read_image),
                static_tool_with_metadata("read_video", "Read and analyze a video from a URL through a configured fallback model.", tool_metadata("host_operations", false, false), read_video),
                static_tool_with_metadata("read_audio", "Read and analyze audio from a URL through a configured fallback model.", tool_metadata("host_operations", false, false), read_audio),
                static_tool_with_metadata("load_media_url", "Load multimedia content directly from an HTTP or HTTPS URL when model capabilities support it.", tool_metadata("host_operations", false, false), load_media_url),
                static_tool_with_metadata("summarize", "Summarize current work and return a handoff envelope.", tool_metadata("host_operations", true, false), summarize),
                static_tool_with_metadata("note", "Create, update, or delete a note entry.", tool_metadata("host_operations", true, false), note_set),
                static_tool_with_metadata("note_get", "Read note entries by key, or list all note entries.", tool_metadata("host_operations", true, false), note_get),
                static_tool_with_metadata("thinking", "Record a thinking or reasoning note for the host.", tool_metadata("host_operations", true, false), thinking),
            ]),
    )
}

fn json_result(value: impl Serialize, tool: &str) -> Result<ToolResult, ToolError> {
    serde_json::to_value(value)
        .map(ToolResult::new)
        .map_err(|error| tool_execution_error(tool, error))
}
