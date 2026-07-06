use reqwest::header;
use serde_json::Value;
use starweaver_tools::{ToolError, ToolResult};

use super::{SearchRequest, SearchResponse, SearchResultItem};
use crate::bundles::helpers::{tool_execution_error, tool_feedback};

use super::super::{http::first_env, http::http_client, json_result};

pub(super) async fn brave_search(request: SearchRequest) -> Result<ToolResult, ToolError> {
    let Some(key) = first_env(["BRAVE_SEARCH_API_KEY", "BRAVE_API_KEY"]) else {
        return Err(tool_feedback(
            "search",
            "No search API key configured. Set BRAVE_SEARCH_API_KEY or BRAVE_API_KEY, or provide a HostSearchClientHandle search adapter.",
        ));
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
        return Err(tool_feedback(
            "search",
            format!(
                "Brave Search returned HTTP {status}: {value}. Refine the query, verify API credentials, or use a configured HostSearchClientHandle search adapter."
            ),
        ));
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

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}
