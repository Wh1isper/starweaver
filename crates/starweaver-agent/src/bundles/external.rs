use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_tools::{
    DynToolset, EmptyToolArgs, StaticToolset, ToolContext, ToolError, ToolInstruction, ToolResult,
};

use super::helpers::static_tool;

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
                static_tool("search", "Search the web for information using search APIs.", search),
                static_tool("search_stock_image", "Search royalty-free stock images from Pixabay for design work.", search_stock_image),
                static_tool("search_image", "Search real-time images via an image-search provider.", search_image),
                static_tool("fetch", "Read web files or check resource availability via HTTP.", fetch),
                static_tool("scrape", "Convert websites to Markdown format for content analysis.", scrape),
                static_tool("download", "Download files from URLs and save to local filesystem.", download),
                static_tool("pdf_convert", "Convert PDF to markdown with image extraction.", pdf_convert),
                static_tool("office_to_markdown", "Convert Office documents and EPub to markdown.", office_to_markdown),
                static_tool("read_image", "Read and analyze an image from a URL.", read_image),
                static_tool("read_video", "Read and analyze a video from a URL.", read_video),
                static_tool("read_audio", "Read and analyze audio from a URL.", read_audio),
                static_tool("load_media_url", "Load multimedia content directly from an HTTP or HTTPS URL.", load_media_url),
                static_tool("summarize", "Summarize current work and return a handoff envelope.", summarize),
                static_tool("note", "Create, update, or delete a note entry.", note_set),
                static_tool("note_get", "Read note entries by key, or list all note entries.", note_get),
                static_tool("thinking", "Record a thinking or reasoning note for the host.", thinking),
                static_tool("to_do_read", "Read the current session to-do list operation envelope.", to_do_read),
                static_tool("to_do_write", "Replace the current session to-do list operation envelope.", to_do_write),
            ]),
    )
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchArgs {
    query: String,
    #[serde(default = "default_search_num")]
    num: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchStockImageArgs {
    query: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SearchImageArgs {
    query: String,
    #[serde(default = "default_image_limit")]
    limit: u8,
    #[serde(default = "default_image_size")]
    size: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct UrlArgs {
    url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FetchArgs {
    url: String,
    #[serde(default)]
    head_only: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct DownloadArgs {
    urls: Vec<String>,
    save_dir: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct PdfConvertArgs {
    file_path: String,
    page_start: Option<usize>,
    page_end: Option<isize>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct FilePathArgs {
    file_path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct SummarizeArgs {
    content: String,
    auto_load_files: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct NoteSetArgs {
    key: String,
    value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct NoteGetArgs {
    key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct ThinkingArgs {
    content: String,
    metadata: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TodoItem {
    id: String,
    content: String,
    status: String,
    priority: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct TodoWriteArgs {
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
        serde_json::json!({"content": arguments.content, "metadata": arguments.metadata.unwrap_or_else(|| serde_json::json!({}))}),
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
