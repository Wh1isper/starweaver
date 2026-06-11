use std::sync::Arc;

use async_trait::async_trait;
use reqwest::{header, Method};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_context::AgentContext;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    args::{FetchArgs, SearchArgs, UrlArgs},
    http::{
        fetch_http_resource, first_env, http_client, is_text_like, truncate_text,
        validate_http_url, MAX_FETCH_BYTES,
    },
    json_result,
    media::{classify_media, document_handoff, MediaKind},
};
use crate::{
    bundles::helpers::tool_execution_error,
    media_compression::{compress_image_to_model_limit, data_url, raw_budget_for_encoded_limit},
};

/// Search client request used by injectable host search adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchRequest {
    /// Search query.
    pub query: String,
    /// Requested result count.
    pub num: u8,
}

/// Normalized host search result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchResultItem {
    /// Result title.
    pub title: String,
    /// Result URL.
    pub url: String,
    /// Result snippet or description.
    pub description: String,
    /// Source search provider.
    pub provider: String,
    /// One-based provider-normalized rank.
    pub rank: usize,
    /// Optional content type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Optional published time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Optional citation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<Value>,
}

/// Normalized host search response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SearchResponse {
    /// Whether search succeeded.
    pub success: bool,
    /// Original query.
    pub query: String,
    /// Normalized results.
    #[serde(default)]
    pub results: Vec<SearchResultItem>,
    /// Provider errors that did not abort tool execution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// Whether provider results were truncated.
    pub truncated: bool,
    /// Source provider name.
    pub provider: String,
}

/// Injectable host search adapter.
#[async_trait]
pub trait HostSearchClient: Send + Sync {
    /// Execute a normalized search request.
    async fn search(&self, request: SearchRequest) -> Result<SearchResponse, String>;
}

/// Typed dependency wrapper for an injectable host search adapter.
#[derive(Clone)]
pub struct HostSearchClientHandle {
    pub(super) client: Arc<dyn HostSearchClient>,
}

impl HostSearchClientHandle {
    /// Create a search client handle.
    #[must_use]
    pub fn new(client: Arc<dyn HostSearchClient>) -> Self {
        Self { client }
    }
}

/// Scrape client request used by injectable host scrape adapters.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScrapeRequest {
    /// URL to scrape.
    pub url: String,
}

/// Normalized scrape response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScrapeResponse {
    /// Whether scraping succeeded.
    pub success: bool,
    /// Original URL.
    pub url: String,
    /// Final URL after redirects.
    pub final_url: String,
    /// Optional page title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Markdown content.
    #[serde(default)]
    pub markdown_content: String,
    /// Adapter that produced the result.
    pub adapter: String,
    /// Whether content was truncated.
    pub truncated: bool,
    /// Total untruncated character length.
    pub total_length: usize,
    /// Optional content type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Optional citation metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation: Option<Value>,
    /// Optional handoff guidance for resources handled by other tools or skills.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Value>,
}

/// Injectable host scrape adapter.
#[async_trait]
pub trait HostScrapeClient: Send + Sync {
    /// Execute a normalized scrape request.
    async fn scrape(&self, request: ScrapeRequest) -> Result<ScrapeResponse, String>;
}

/// Typed dependency wrapper for an injectable host scrape adapter.
#[derive(Clone)]
pub struct HostScrapeClientHandle {
    pub(super) client: Arc<dyn HostScrapeClient>,
}

impl HostScrapeClientHandle {
    /// Create a scrape client handle.
    #[must_use]
    pub fn new(client: Arc<dyn HostScrapeClient>) -> Self {
        Self { client }
    }
}

pub(super) async fn search(
    context: ToolContext,
    arguments: SearchArgs,
) -> Result<ToolResult, ToolError> {
    let request = SearchRequest {
        query: arguments.query.trim().to_string(),
        num: arguments.num.clamp(1, 20),
    };
    if request.query.is_empty() {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "query": request.query,
            "results": [],
            "errors": ["query must not be empty"],
            "truncated": false,
            "provider": "none",
        })));
    }
    if let Some(handle) = context.dependency::<HostSearchClientHandle>() {
        let response = handle
            .client
            .search(request)
            .await
            .map_err(|error| tool_execution_error("search", error))?;
        return json_result(response, "search");
    }
    brave_search(request).await
}

pub(super) async fn fetch(
    context: ToolContext,
    arguments: FetchArgs,
) -> Result<ToolResult, ToolError> {
    let method = if arguments.head_only {
        Method::HEAD
    } else {
        Method::GET
    };
    let max_fetch_bytes = context
        .dependency::<AgentContext>()
        .map_or(MAX_FETCH_BYTES, |context| {
            context
                .tool_config
                .fetch_max_inline_binary_bytes
                .max(MAX_FETCH_BYTES)
        });
    let resource =
        fetch_http_resource(&context, "fetch", &arguments.url, method, max_fetch_bytes).await?;
    if arguments.head_only {
        return Ok(ToolResult::new(serde_json::json!({
            "success": (200..400).contains(&resource.status),
            "url": arguments.url,
            "final_url": resource.final_url,
            "status": resource.status,
            "content_type": resource.content_type,
            "content_length": resource.content_length,
        })));
    }
    let Some(body) = resource.body.clone() else {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "url": arguments.url,
            "final_url": resource.final_url,
            "status": resource.status,
            "content_type": resource.content_type,
            "content_length": resource.content_length,
            "error": "response body was not loaded",
        })));
    };
    if resource
        .content_type
        .as_deref()
        .is_some_and(|content_type| content_type.to_ascii_lowercase().contains("image"))
    {
        return Ok(fetch_image_result(
            &context,
            &arguments.url,
            &resource,
            body,
        ));
    }
    if !is_text_like(resource.content_type.as_deref()) && std::str::from_utf8(&body).is_err() {
        return Ok(ToolResult::new(serde_json::json!({
            "success": (200..400).contains(&resource.status),
            "url": arguments.url,
            "final_url": resource.final_url,
            "status": resource.status,
            "content_type": resource.content_type,
            "content_length": resource.content_length,
            "binary": true,
            "message": "binary content is available through download or media tools",
        })));
    }
    let text = String::from_utf8_lossy(&body);
    let (text_content, truncated, total_length) = truncate_text(&text);
    Ok(ToolResult::new(serde_json::json!({
        "success": (200..400).contains(&resource.status),
        "url": arguments.url,
        "final_url": resource.final_url,
        "status": resource.status,
        "content_type": resource.content_type,
        "content_length": resource.content_length,
        "content": text_content,
        "total_length": total_length,
        "truncated": truncated,
    })))
}

fn fetch_image_result(
    context: &ToolContext,
    requested_url: &str,
    resource: &super::http::HttpResource,
    mut body: Vec<u8>,
) -> ToolResult {
    let mut media_type = starweaver_model::detect_media_kind(&body)
        .media_type()
        .unwrap_or_else(|| {
            resource
                .content_type
                .as_deref()
                .and_then(|content_type| content_type.split(';').next())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("image/jpeg")
        })
        .to_string();
    let original_bytes = body.len();
    let mut compressed_for_model = false;
    if let Some(agent_context) = context.dependency::<AgentContext>() {
        let max_image_bytes = agent_context.model_config.max_image_bytes;
        if max_image_bytes > 0 && body.len() > raw_budget_for_encoded_limit(max_image_bytes) {
            match compress_image_to_model_limit(&body, max_image_bytes, &media_type) {
                Ok(compressed) => {
                    if compressed.data.len() > raw_budget_for_encoded_limit(max_image_bytes) {
                        return ToolResult::new(serde_json::json!({
                            "success": false,
                            "url": requested_url,
                            "final_url": resource.final_url,
                            "status": resource.status,
                            "content_type": resource.content_type,
                            "content_length": resource.content_length,
                            "error": format!(
                                "Fetched image could not be compressed below the {max_image_bytes} byte API limit after accounting for base64 encoding."
                            ),
                        }));
                    }
                    body = compressed.data;
                    media_type = compressed.media_type;
                    compressed_for_model = compressed.compressed;
                }
                Err(error) => {
                    return ToolResult::new(serde_json::json!({
                        "success": false,
                        "url": requested_url,
                        "final_url": resource.final_url,
                        "status": resource.status,
                        "content_type": resource.content_type,
                        "content_length": resource.content_length,
                        "error": "Fetched image could not be compressed for inline model input.",
                        "details": error,
                    }));
                }
            }
        }
    }
    let mut private_metadata = Map::new();
    private_metadata.insert(
        "starweaver_tool_return_content_parts".to_string(),
        serde_json::json!([{
            "kind": "data_url",
            "data_url": data_url(&media_type, &body),
            "media_type": media_type,
        }]),
    );
    private_metadata.insert(
        "starweaver_tool_return_prompt".to_string(),
        serde_json::json!("The fetch tool loaded an image from the requested URL. Inspect the attached image and answer accordingly."),
    );
    ToolResult::new(serde_json::json!({
        "success": (200..400).contains(&resource.status),
        "url": requested_url,
        "final_url": resource.final_url,
        "status": resource.status,
        "content_type": resource.content_type,
        "content_length": resource.content_length,
        "media_type": media_type,
        "binary": true,
        "message": "The image is attached in a provider-native media message.",
        "compressed": compressed_for_model,
        "original_bytes": original_bytes,
        "inline_bytes": body.len(),
    }))
    .with_private_metadata(private_metadata)
    .with_model_content(serde_json::json!(
        "The image is attached in the user message."
    ))
}

pub(super) async fn scrape(
    context: ToolContext,
    arguments: UrlArgs,
) -> Result<ToolResult, ToolError> {
    if let Some(handle) = context.dependency::<HostScrapeClientHandle>() {
        let response = handle
            .client
            .scrape(ScrapeRequest { url: arguments.url })
            .await
            .map_err(|error| tool_execution_error("scrape", error))?;
        return json_result(response, "scrape");
    }
    if let Some(key) = first_env(["FIRECRAWL_API_KEY"]) {
        if let Ok(response) = firecrawl_scrape(&context, &arguments.url, &key).await {
            return json_result(response, "scrape");
        }
    }
    if let Some(token) = first_env(["CLOUDFLARE_API_TOKEN"]) {
        if let Ok(response) = cloudflare_scrape(&context, &arguments.url, &token) {
            return json_result(response, "scrape");
        }
    }
    local_scrape(&context, &arguments.url).await
}

async fn brave_search(request: SearchRequest) -> Result<ToolResult, ToolError> {
    let Some(key) = first_env(["BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY"]) else {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "query": request.query,
            "results": [],
            "errors": ["No search API key configured"],
            "missing_env": ["BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY"],
            "truncated": false,
            "provider": "brave",
        })));
    };
    let client = http_client("search")?;
    let response = client
        .get("https://api.search.brave.com/res/v1/web/search")
        .header("X-Subscription-Token", key)
        .header(header::ACCEPT, "application/json")
        .query(&[
            ("q", request.query.as_str()),
            ("count", &request.num.to_string()),
        ])
        .send()
        .await
        .map_err(|error| tool_execution_error("search", error))?;
    let status = response.status();
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| tool_execution_error("search", error))?;
    if !status.is_success() {
        return Ok(ToolResult::new(serde_json::json!({
            "success": false,
            "query": request.query,
            "results": [],
            "errors": [format!("Brave Search returned HTTP {status}")],
            "provider": "brave",
            "truncated": false,
            "metadata": value,
        })));
    }
    let mut results = Vec::new();
    if let Some(items) = value.pointer("/web/results").and_then(Value::as_array) {
        for (index, item) in items.iter().take(usize::from(request.num)).enumerate() {
            results.push(SearchResultItem {
                title: string_field(item, "title"),
                url: string_field(item, "url"),
                description: string_field(item, "description"),
                provider: "brave".to_string(),
                rank: index + 1,
                content_type: item
                    .get("content_type")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                published_at: item
                    .get("age")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                citation: Some(serde_json::json!({"provider": "brave", "rank": index + 1})),
            });
        }
    }
    json_result(
        SearchResponse {
            success: true,
            query: request.query,
            truncated: value
                .pointer("/web/results")
                .and_then(Value::as_array)
                .is_some_and(|items| items.len() > results.len()),
            results,
            errors: Vec::new(),
            provider: "brave".to_string(),
        },
        "search",
    )
}

async fn firecrawl_scrape(
    context: &ToolContext,
    url: &str,
    key: &str,
) -> Result<ScrapeResponse, ToolError> {
    validate_http_url(context, "scrape", url)?;
    let client = http_client("scrape")?;
    let response = client
        .post("https://api.firecrawl.dev/v1/scrape")
        .bearer_auth(key)
        .json(&serde_json::json!({"url": url, "formats": ["markdown"]}))
        .send()
        .await
        .map_err(|error| tool_execution_error("scrape", error))?;
    let status = response.status();
    let final_url = response.url().to_string();
    let value = response
        .json::<Value>()
        .await
        .map_err(|error| tool_execution_error("scrape", error))?;
    if !status.is_success() || value.get("success").and_then(Value::as_bool) == Some(false) {
        return Err(tool_execution_error(
            "scrape",
            format!("Firecrawl returned HTTP {status}"),
        ));
    }
    let data = value.get("data").unwrap_or(&value);
    let markdown = data
        .get("markdown")
        .or_else(|| data.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (markdown_content, truncated, total_length) = truncate_text(markdown);
    let metadata = data.get("metadata").unwrap_or(&Value::Null);
    Ok(ScrapeResponse {
        success: true,
        url: url.to_string(),
        final_url: metadata
            .get("sourceURL")
            .and_then(Value::as_str)
            .unwrap_or(&final_url)
            .to_string(),
        title: metadata
            .get("title")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        markdown_content,
        adapter: "firecrawl".to_string(),
        truncated,
        total_length,
        content_type: None,
        citation: Some(serde_json::json!({"url": url, "adapter": "firecrawl"})),
        handoff: None,
    })
}

fn cloudflare_scrape(
    context: &ToolContext,
    url: &str,
    _token: &str,
) -> Result<ScrapeResponse, ToolError> {
    validate_http_url(context, "scrape", url)?;
    Err(tool_execution_error(
        "scrape",
        "Cloudflare scrape adapter is not configured in this SDK build",
    ))
}

async fn local_scrape(context: &ToolContext, url: &str) -> Result<ToolResult, ToolError> {
    let resource =
        fetch_http_resource(context, "scrape", url, Method::GET, MAX_FETCH_BYTES).await?;
    let kind = classify_media(resource.content_type.as_deref(), url);
    if matches!(
        kind,
        MediaKind::Document | MediaKind::Audio | MediaKind::Video | MediaKind::Image
    ) {
        return json_result(
            ScrapeResponse {
                success: false,
                url: url.to_string(),
                final_url: resource.final_url,
                title: None,
                markdown_content: String::new(),
                adapter: "local_static_html".to_string(),
                truncated: false,
                total_length: 0,
                content_type: resource.content_type,
                citation: None,
                handoff: Some(document_handoff(url)),
            },
            "scrape",
        );
    }
    let body = resource.body.unwrap_or_default();
    let text = String::from_utf8_lossy(&body);
    let title = extract_title(&text);
    let markdown = if is_html(resource.content_type.as_deref(), &text) {
        html_to_markdown(&text)
    } else {
        text.to_string()
    };
    let (markdown_content, truncated, total_length) = truncate_text(&markdown);
    json_result(
        ScrapeResponse {
            success: (200..400).contains(&resource.status),
            url: url.to_string(),
            final_url: resource.final_url,
            title,
            markdown_content,
            adapter: "local_static_html".to_string(),
            truncated,
            total_length,
            content_type: resource.content_type,
            citation: Some(serde_json::json!({"url": url, "adapter": "local_static_html"})),
            handoff: None,
        },
        "scrape",
    )
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_html(content_type: Option<&str>, body: &str) -> bool {
    content_type.is_some_and(|content_type| content_type.to_ascii_lowercase().contains("html"))
        || body.to_ascii_lowercase().contains("<html")
        || body.to_ascii_lowercase().contains("<!doctype html")
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let tag_end = lower[start..].find('>')? + start + 1;
    let end = lower[tag_end..].find("</title>")? + tag_end;
    Some(
        decode_basic_entities(html[tag_end..end].trim())
            .trim()
            .to_string(),
    )
    .filter(|title| !title.is_empty())
}

fn html_to_markdown(html: &str) -> String {
    let without_blocks = remove_html_block(html, "script");
    let without_blocks = remove_html_block(&without_blocks, "style");
    let mut output = String::new();
    let mut tag = String::new();
    let mut in_tag = false;
    for character in without_blocks.chars() {
        if in_tag {
            if character == '>' {
                append_tag_boundary(&mut output, &tag);
                tag.clear();
                in_tag = false;
            } else {
                tag.push(character);
            }
        } else if character == '<' {
            in_tag = true;
        } else {
            output.push(character);
        }
    }
    collapse_markdown_whitespace(&decode_basic_entities(&output))
}

fn remove_html_block(input: &str, tag: &str) -> String {
    let mut output = String::new();
    let mut remaining = input;
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    loop {
        let lower = remaining.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            output.push_str(remaining);
            break;
        };
        output.push_str(&remaining[..start]);
        let Some(end) = lower[start..].find(&close) else {
            break;
        };
        remaining = &remaining[start + end + close.len()..];
    }
    output
}

fn append_tag_boundary(output: &mut String, raw_tag: &str) {
    let tag = raw_tag
        .trim()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match tag.as_str() {
        "br" | "p" | "div" | "section" | "article" | "header" | "footer" | "tr" => {
            output.push('\n');
        }
        "li" => output.push_str("\n- "),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => output.push_str("\n\n"),
        _ => {}
    }
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn collapse_markdown_whitespace(input: &str) -> String {
    let mut output = String::new();
    let mut blank_lines = 0_usize;
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_lines += 1;
            if blank_lines <= 1 && !output.is_empty() {
                output.push('\n');
            }
        } else {
            blank_lines = 0;
            if !output.is_empty() && !output.ends_with('\n') {
                output.push('\n');
            }
            output.push_str(trimmed);
            output.push('\n');
        }
    }
    output.trim().to_string()
}
