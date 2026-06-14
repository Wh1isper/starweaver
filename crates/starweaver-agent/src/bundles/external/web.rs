use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::{
    args::{FetchArgs, SearchArgs, UrlArgs},
    http::{fetch_http_resource, first_env, is_text_like, truncate_text, MAX_FETCH_BYTES},
    json_result,
};
use crate::bundles::helpers::tool_execution_error;

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
