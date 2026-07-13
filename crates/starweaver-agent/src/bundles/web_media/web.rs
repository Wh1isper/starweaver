use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::ToolRuntimeSnapshot;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    args::{FetchArgs, SearchArgs, UrlArgs},
    http::{MAX_FETCH_BYTES, fetch_http_resource, first_env, is_text_like},
    json_result,
};
use crate::bundles::helpers::{tool_execution_error, tool_feedback, tool_invalid_arguments};
use crate::bundles::output::{
    DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT, append_guidance, dump_tool_output,
    environment_provider_from_context, fit_text_fields_to_limit, output_too_large_message,
    tool_output_size, write_tmp_output,
};

mod fetch_image;
mod html;
mod scrape_impl;
mod search_provider;

use fetch_image::fetch_image_result;
use scrape_impl::{cloudflare_scrape, firecrawl_scrape, local_scrape};
use search_provider::brave_search;

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
        return Err(tool_invalid_arguments(
            "search",
            "query must not be empty. Provide a concise search query, or skip search if there is nothing to look up.",
        ));
    }
    if let Some(handle) = context.dependency::<HostSearchClientHandle>() {
        let response = handle
            .client
            .search(request)
            .await
            .map_err(|error| tool_execution_error("search", error))?;
        let result = json_result(response, "search")?;
        return Ok(ToolResult::new(
            guard_search_result(&context, result.content, "search").await,
        ));
    }
    let result = brave_search(request).await?;
    Ok(ToolResult::new(
        guard_search_result(&context, result.content, "search").await,
    ))
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
    let max_fetch_bytes =
        context
            .dependency::<ToolRuntimeSnapshot>()
            .map_or(MAX_FETCH_BYTES, |runtime| {
                runtime
                    .tool_config()
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
        return Err(tool_execution_error(
            "fetch",
            format!(
                "response body was not loaded for {} after HTTP status {}",
                resource.final_url, resource.status
            ),
        ));
    };
    if !(200..400).contains(&resource.status) {
        return Err(tool_feedback(
            "fetch",
            format!(
                "HTTP {} returned for {}. Verify the URL, authentication, and whether the resource exists. Use head_only=true if you only need availability metadata.",
                resource.status, resource.final_url
            ),
        ));
    }
    if resource
        .content_type
        .as_deref()
        .is_some_and(|content_type| content_type.to_ascii_lowercase().contains("image"))
    {
        return fetch_image_result(&context, &arguments.url, &resource, body);
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
    let total_length = text.chars().count();
    let result = serde_json::json!({
        "success": (200..400).contains(&resource.status),
        "url": arguments.url,
        "final_url": resource.final_url,
        "status": resource.status,
        "content_type": resource.content_type,
        "content_length": resource.content_length,
        "content": text,
        "total_length": total_length,
        "truncated": false,
    });
    Ok(ToolResult::new(
        guard_text_result(
            &context,
            result,
            "fetch",
            "txt",
            "content",
            "content",
            "\n\n... (truncated; full content saved in `output_file_path`)",
        )
        .await,
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
        let result = json_result(response, "scrape")?;
        return Ok(ToolResult::new(
            guard_scrape_result(&context, result.content).await,
        ));
    }
    if let Some(key) = first_env(["FIRECRAWL_API_KEY"])
        && let Ok(response) = firecrawl_scrape(&context, &arguments.url, &key).await
    {
        let result = json_result(response, "scrape")?;
        return Ok(ToolResult::new(
            guard_scrape_result(&context, result.content).await,
        ));
    }
    if let Some(token) = first_env(["CLOUDFLARE_API_TOKEN"])
        && let Ok(response) = cloudflare_scrape(&context, &arguments.url, &token)
    {
        let result = json_result(response, "scrape")?;
        return Ok(ToolResult::new(
            guard_scrape_result(&context, result.content).await,
        ));
    }
    let result = local_scrape(&context, &arguments.url).await?;
    Ok(ToolResult::new(
        guard_scrape_result(&context, result.content).await,
    ))
}

async fn guard_scrape_result(context: &ToolContext, result: Value) -> Value {
    guard_text_result(
        context,
        result,
        "scrape",
        "md",
        "markdown_content",
        "Markdown",
        "\n\n... (truncated; full Markdown saved in `output_file_path`)",
    )
    .await
}

async fn guard_text_result(
    context: &ToolContext,
    result: Value,
    prefix: &str,
    extension: &str,
    text_field: &str,
    noun: &str,
    suffix: &str,
) -> Value {
    if tool_output_size(&result) <= DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT {
        return result;
    }
    let full_text = result
        .get(text_field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let provider = environment_provider_from_context(context);
    let output_path =
        write_tmp_output(provider.as_deref(), prefix, extension, full_text.as_bytes()).await;
    let mut preview = match result {
        Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("result".to_string(), other);
            map
        }
    };
    preview.insert("truncated".to_string(), Value::Bool(true));
    if let Some(path) = output_path.as_ref() {
        preview.insert("output_file_path".to_string(), Value::String(path.clone()));
    }
    let size = preview
        .get("total_length")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_else(|| full_text.chars().count());
    let guidance = output_too_large_message(size, output_path.as_deref(), noun);
    preview.insert("tips".to_string(), Value::String(guidance));
    fit_text_fields_to_limit(
        Value::Object(preview),
        &[text_field],
        DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT,
        suffix,
    )
}

async fn guard_search_result(context: &ToolContext, result: Value, prefix: &str) -> Value {
    if tool_output_size(&result) <= DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT {
        return result;
    }
    let serialized = dump_tool_output(&result);
    let provider = environment_provider_from_context(context);
    let output_path =
        write_tmp_output(provider.as_deref(), prefix, "json", serialized.as_bytes()).await;
    let note = output_too_large_message(
        serialized.chars().count(),
        output_path.as_deref(),
        "search results",
    );

    let Value::Object(map) = result else {
        let mut preview = serde_json::Map::new();
        preview.insert("truncated".to_string(), Value::Bool(true));
        preview.insert("note".to_string(), Value::String(note));
        if let Some(path) = output_path {
            preview.insert("output_file_path".to_string(), Value::String(path));
        }
        return Value::Object(preview);
    };

    let results = map
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut preview = map;
    let existing_note = preview
        .get("note")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    preview.insert(
        "note".to_string(),
        Value::String(append_guidance(existing_note.as_deref(), &note)),
    );
    preview.insert("truncated".to_string(), Value::Bool(true));
    preview.insert("results".to_string(), Value::Array(Vec::new()));
    preview.insert(
        "results_total".to_string(),
        serde_json::json!(results.len()),
    );
    preview.insert("results_showing".to_string(), serde_json::json!(0));
    if let Some(path) = output_path.as_ref() {
        preview.insert("output_file_path".to_string(), Value::String(path.clone()));
    }

    if tool_output_size(&Value::Object(preview.clone())) > DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT {
        let mut minimal = serde_json::Map::new();
        for key in ["success", "query", "provider", "errors"] {
            if let Some(value) = preview.get(key) {
                minimal.insert(key.to_string(), value.clone());
            }
        }
        minimal.insert("truncated".to_string(), Value::Bool(true));
        minimal.insert("note".to_string(), Value::String(note));
        minimal.insert(
            "results_total".to_string(),
            serde_json::json!(results.len()),
        );
        minimal.insert("results_showing".to_string(), serde_json::json!(0));
        if let Some(path) = output_path {
            minimal.insert("output_file_path".to_string(), Value::String(path));
        }
        preview = minimal;
    }

    let mut kept = Vec::new();
    for item in results {
        kept.push(item);
        let mut candidate = preview.clone();
        candidate.insert("results".to_string(), Value::Array(kept.clone()));
        candidate.insert("results_showing".to_string(), serde_json::json!(kept.len()));
        if tool_output_size(&Value::Object(candidate.clone())) > DEFAULT_TOOL_OUTPUT_TRUNCATE_LIMIT
        {
            kept.pop();
            break;
        }
        preview = candidate;
    }
    preview.insert("results".to_string(), Value::Array(kept.clone()));
    preview.insert("results_showing".to_string(), serde_json::json!(kept.len()));
    Value::Object(preview)
}
