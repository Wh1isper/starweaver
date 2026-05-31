use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::{static_tool_with_metadata, tool_metadata};

/// Create host-operation tools for web, document, media, and context-management capabilities.
#[must_use]
pub fn host_operation_tools() -> DynToolset {
    Arc::new(
        StaticToolset::new("host_operations")
            .with_id("host_operations")
            .with_instruction(ToolInstruction::new(
                "host_operations",
                "These tools expose operation envelopes for host-provided capabilities such as web access, image search, document conversion, media analysis, notes, thinking, todos, and context handoff.",
            ))
            .with_tools([
                static_tool_with_metadata("search", "Search the web for information using search APIs.", tool_metadata("host_operations", false, false), search),
                static_tool_with_metadata("search_stock_image", "Search royalty-free stock images from Pixabay for design work.", tool_metadata("host_operations", false, false), search_stock_image),
                static_tool_with_metadata("search_image", "Search real-time images via an image-search provider.", tool_metadata("host_operations", false, false), search_image),
                static_tool_with_metadata("fetch", "Read web files or check resource availability via HTTP.", tool_metadata("host_operations", false, false), fetch),
                static_tool_with_metadata("scrape", "Convert websites to Markdown format for content analysis.", tool_metadata("host_operations", false, false), scrape),
                static_tool_with_metadata("download", "Download files from URLs and save to local filesystem.", tool_metadata("host_operations", false, false), download),
                static_tool_with_metadata("pdf_convert", "Convert PDF to markdown with image extraction.", tool_metadata("host_operations", false, false), pdf_convert),
                static_tool_with_metadata("office_to_markdown", "Convert Office documents and EPub to markdown.", tool_metadata("host_operations", false, false), office_to_markdown),
                static_tool_with_metadata("read_image", "Read and analyze an image from a URL.", tool_metadata("host_operations", false, false), read_image),
                static_tool_with_metadata("read_video", "Read and analyze a video from a URL.", tool_metadata("host_operations", false, false), read_video),
                static_tool_with_metadata("read_audio", "Read and analyze audio from a URL.", tool_metadata("host_operations", false, false), read_audio),
                static_tool_with_metadata("load_media_url", "Load multimedia content directly from an HTTP or HTTPS URL.", tool_metadata("host_operations", false, false), load_media_url),
                static_tool_with_metadata("summarize", "Summarize current work and return a handoff envelope.", tool_metadata("host_operations", true, false), summarize),
                static_tool_with_metadata("note", "Create, update, or delete a note entry.", tool_metadata("host_operations", true, false), note_set),
                static_tool_with_metadata("note_get", "Read note entries by key, or list all note entries.", tool_metadata("host_operations", true, false), note_get),
                static_tool_with_metadata("thinking", "Record a thinking or reasoning note for the host.", tool_metadata("host_operations", true, false), thinking),
                static_tool_with_metadata("to_do_read", "Read the current session to-do list operation envelope.", tool_metadata("host_operations", true, false), to_do_read),
                static_tool_with_metadata("to_do_write", "Replace the current session to-do list operation envelope.", tool_metadata("host_operations", true, false), to_do_write),
            ]),
    )
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchArgs {
    /// The search query.
    query: String,
    /// Number of results to return.
    #[serde(default = "default_search_num")]
    num: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchStockImageArgs {
    /// Search term for stock images.
    query: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchImageArgs {
    /// Search query or keywords.
    query: String,
    /// Maximum results to return.
    #[serde(default = "default_image_limit")]
    limit: u8,
    /// Image size such as any, large, medium, or icon.
    #[serde(default = "default_image_size")]
    size: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct UrlArgs {
    /// URL of the resource.
    url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FetchArgs {
    /// URL of the web resource to fetch.
    url: String,
    /// Only check existence without downloading content.
    #[serde(default)]
    head_only: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct DownloadArgs {
    /// List of URLs to download.
    urls: Vec<String>,
    /// Directory where files should be saved.
    save_dir: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PdfConvertArgs {
    /// Path to the PDF file to convert.
    file_path: String,
    /// Starting page number, 1-based.
    #[serde(default)]
    page_start: Option<usize>,
    /// Ending page number, 1-based inclusive. Use -1 for all pages.
    #[serde(default)]
    page_end: Option<isize>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FilePathArgs {
    /// Path to the file.
    file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SummarizeArgs {
    /// Context summary to preserve across context handoff.
    content: String,
    /// File paths to auto-load after summary.
    #[serde(default)]
    auto_load_files: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct NoteSetArgs {
    /// Unique key for the note entry.
    key: String,
    /// Content to store. Omit or set to null to delete the entry.
    #[serde(default)]
    value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct NoteGetArgs {
    /// The note key to retrieve. Omit to list all notes.
    #[serde(default)]
    key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ThinkingArgs {
    /// A thought in markdown format.
    #[serde(alias = "content")]
    thought: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TodoItem {
    /// Stable to-do item ID.
    id: String,
    /// To-do item content.
    content: String,
    /// To-do item status.
    status: String,
    /// To-do item priority.
    priority: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TodoWriteArgs {
    /// The updated to-do list.
    to_dos: Vec<TodoItem>,
}

const fn default_search_num() -> u8 {
    10
}

const fn default_image_limit() -> u8 {
    10
}

fn default_image_size() -> String {
    "any".to_string()
}

async fn search(_context: ToolContext, arguments: SearchArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "search",
        serde_json::json!({"query": arguments.query, "num": arguments.num}),
    ))
}

async fn search_stock_image(
    _context: ToolContext,
    arguments: SearchStockImageArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "search_stock_image",
        serde_json::json!({"query": arguments.query}),
    ))
}

async fn search_image(
    _context: ToolContext,
    arguments: SearchImageArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "search_image",
        serde_json::json!({
            "query": arguments.query,
            "limit": arguments.limit,
            "size": arguments.size,
        }),
    ))
}

async fn fetch(_context: ToolContext, arguments: FetchArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "fetch",
        serde_json::json!({"url": arguments.url, "head_only": arguments.head_only}),
    ))
}

async fn scrape(_context: ToolContext, arguments: UrlArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "scrape",
        serde_json::json!({"url": arguments.url}),
    ))
}

async fn download(_context: ToolContext, arguments: DownloadArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "download",
        serde_json::json!({"urls": arguments.urls, "save_dir": arguments.save_dir}),
    ))
}

async fn pdf_convert(
    _context: ToolContext,
    arguments: PdfConvertArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "pdf_convert",
        serde_json::json!({
            "file_path": arguments.file_path,
            "page_start": arguments.page_start,
            "page_end": arguments.page_end,
        }),
    ))
}

async fn office_to_markdown(
    _context: ToolContext,
    arguments: FilePathArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "office_to_markdown",
        serde_json::json!({"file_path": arguments.file_path}),
    ))
}

async fn read_image(_context: ToolContext, arguments: UrlArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "read_image",
        serde_json::json!({"url": arguments.url}),
    ))
}

async fn read_video(_context: ToolContext, arguments: UrlArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "read_video",
        serde_json::json!({"url": arguments.url}),
    ))
}

async fn read_audio(_context: ToolContext, arguments: UrlArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "read_audio",
        serde_json::json!({"url": arguments.url}),
    ))
}

async fn load_media_url(
    _context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "load_media_url",
        serde_json::json!({"url": arguments.url}),
    ))
}

async fn summarize(
    _context: ToolContext,
    arguments: SummarizeArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "summarize",
        serde_json::json!({
            "content": arguments.content,
            "auto_load_files": arguments.auto_load_files.unwrap_or_default(),
        }),
    ))
}

async fn note_set(_context: ToolContext, arguments: NoteSetArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "note",
        serde_json::json!({"key": arguments.key, "value": arguments.value}),
    ))
}

async fn note_get(_context: ToolContext, arguments: NoteGetArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "note_get",
        serde_json::json!({"key": arguments.key}),
    ))
}

async fn thinking(_context: ToolContext, arguments: ThinkingArgs) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "thinking",
        serde_json::json!({"thought": arguments.thought}),
    ))
}

async fn to_do_read(
    _context: ToolContext,
    _arguments: EmptyToolArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation("to_do_read", serde_json::json!({})))
}

async fn to_do_write(
    _context: ToolContext,
    arguments: TodoWriteArgs,
) -> Result<ToolResult, ToolError> {
    Ok(operation(
        "to_do_write",
        serde_json::json!({"to_dos": arguments.to_dos}),
    ))
}

fn operation(name: &str, payload: Value) -> ToolResult {
    let mut content = serde_json::Map::new();
    content.insert("operation".to_string(), Value::String(name.to_string()));
    content.insert("payload".to_string(), payload);
    ToolResult::new(Value::Object(content))
}
