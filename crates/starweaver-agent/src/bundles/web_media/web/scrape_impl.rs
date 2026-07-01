use reqwest::Method;
use serde_json::Value;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use super::ScrapeResponse;
use super::html::{extract_title, html_to_markdown, is_html};
use crate::bundles::helpers::tool_execution_error;

use super::super::{
    http::{MAX_FETCH_BYTES, fetch_http_resource, http_client, validate_http_url},
    json_result,
    media::{MediaKind, classify_media, document_handoff},
};

pub(super) async fn firecrawl_scrape(
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
    let total_length = markdown.chars().count();
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
        markdown_content: markdown.to_string(),
        adapter: "firecrawl".to_string(),
        truncated: false,
        total_length,
        content_type: None,
        citation: Some(serde_json::json!({"url": url, "adapter": "firecrawl"})),
        handoff: None,
    })
}

pub(super) fn cloudflare_scrape(
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

pub(super) async fn local_scrape(
    context: &ToolContext,
    url: &str,
) -> Result<ToolResult, ToolError> {
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
    let total_length = markdown.chars().count();
    json_result(
        ScrapeResponse {
            success: (200..400).contains(&resource.status),
            url: url.to_string(),
            final_url: resource.final_url,
            title,
            markdown_content: markdown,
            adapter: "local_static_html".to_string(),
            truncated: false,
            total_length,
            content_type: resource.content_type,
            citation: Some(serde_json::json!({"url": url, "adapter": "local_static_html"})),
            handoff: None,
        },
        "scrape",
    )
}
