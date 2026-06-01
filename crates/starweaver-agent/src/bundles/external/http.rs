use std::time::Duration;

use reqwest::{header, redirect::Policy, Method, Url};
use starweaver_tools::{ToolContext, ToolError};

use crate::bundles::helpers::tool_execution_error;

pub(super) const MAX_FETCH_BYTES: u64 = 2 * 1024 * 1024;
pub(super) const MAX_DOWNLOAD_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_TIMEOUT_SECONDS: u64 = 20;
const TEXT_TRUNCATION_CHARS: usize = 60_000;

#[derive(Clone, Debug)]
pub(super) struct HttpResource {
    pub(super) final_url: String,
    pub(super) status: u16,
    pub(super) content_type: Option<String>,
    pub(super) content_length: Option<u64>,
    pub(super) body: Option<Vec<u8>>,
}

pub(super) async fn fetch_http_resource(
    context: &ToolContext,
    tool: &str,
    raw_url: &str,
    method: Method,
    max_bytes: u64,
) -> Result<HttpResource, ToolError> {
    let url = validate_http_url(context, tool, raw_url)?;
    let client = http_client(tool)?;
    let mut response = client
        .request(method.clone(), url.clone())
        .send()
        .await
        .map_err(|error| tool_execution_error(tool, error))?;
    if method == Method::HEAD && response.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED {
        response = client
            .request(Method::GET, url)
            .send()
            .await
            .map_err(|error| tool_execution_error(tool, error))?;
    }
    validate_http_url(context, tool, response.url().as_str())?;
    let status = response.status().as_u16();
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_length = response.content_length();
    if method == Method::HEAD || max_bytes == 0 {
        return Ok(HttpResource {
            final_url,
            status,
            content_type,
            content_length,
            body: None,
        });
    }
    if content_length.is_some_and(|length| length > max_bytes) {
        return Err(tool_execution_error(
            tool,
            format!("response exceeds configured {max_bytes} byte limit"),
        ));
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| tool_execution_error(tool, error))?
        .to_vec();
    if u64::try_from(body.len()).map_or(true, |length| length > max_bytes) {
        return Err(tool_execution_error(
            tool,
            format!("response exceeds configured {max_bytes} byte limit"),
        ));
    }
    Ok(HttpResource {
        final_url,
        status,
        content_type,
        content_length,
        body: Some(body),
    })
}

pub(super) fn http_client(tool: &str) -> Result<reqwest::Client, ToolError> {
    reqwest::Client::builder()
        .redirect(Policy::limited(5))
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECONDS))
        .user_agent("starweaver-agent-sdk/0.1")
        .build()
        .map_err(|error| tool_execution_error(tool, error))
}

pub(super) fn validate_http_url(
    _context: &ToolContext,
    tool: &str,
    raw_url: &str,
) -> Result<Url, ToolError> {
    let url = Url::parse(raw_url).map_err(|error| tool_execution_error(tool, error))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(tool_execution_error(
            tool,
            "only http and https URLs are supported",
        ));
    }
    if url.host_str().is_none() {
        return Err(tool_execution_error(tool, "URL host is required"));
    }
    Ok(url)
}

pub(super) fn first_env<const N: usize>(names: [&str; N]) -> Option<String> {
    names.into_iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

pub(super) fn truncate_text(text: &str) -> (String, bool, usize) {
    let total_length = text.chars().count();
    if total_length <= TEXT_TRUNCATION_CHARS {
        return (text.to_string(), false, total_length);
    }
    (
        text.chars().take(TEXT_TRUNCATION_CHARS).collect(),
        true,
        total_length,
    )
}

pub(super) fn is_text_like(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let content_type = content_type.to_ascii_lowercase();
    content_type.starts_with("text/")
        || content_type.contains("json")
        || content_type.contains("xml")
        || content_type.contains("html")
        || content_type.contains("javascript")
        || content_type.contains("x-www-form-urlencoded")
}

pub(super) fn looks_textual(text: &str) -> bool {
    text.chars().take(256).all(|character| {
        character == '\n' || character == '\r' || character == '\t' || !character.is_control()
    })
}

pub(super) fn content_type_from_extension(path: &str) -> Option<String> {
    extension_for_content_type_from_extension(filename_extension(path).as_deref())
}

pub(super) fn extension_for_content_type_from_extension(extension: Option<&str>) -> Option<String> {
    match extension {
        Some("png") => Some("image/png".to_string()),
        Some("jpg" | "jpeg") => Some("image/jpeg".to_string()),
        Some("gif") => Some("image/gif".to_string()),
        Some("webp") => Some("image/webp".to_string()),
        Some("mp4") => Some("video/mp4".to_string()),
        Some("webm") => Some("video/webm".to_string()),
        Some("mov") => Some("video/quicktime".to_string()),
        Some("mp3") => Some("audio/mpeg".to_string()),
        Some("wav") => Some("audio/wav".to_string()),
        Some("ogg") => Some("audio/ogg".to_string()),
        Some("pdf") => Some("application/pdf".to_string()),
        Some("docx") => Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        Some("xlsx") => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string())
        }
        Some("pptx") => Some(
            "application/vnd.openxmlformats-officedocument.presentationml.presentation".to_string(),
        ),
        Some("html" | "htm") => Some("text/html".to_string()),
        Some("txt") => Some("text/plain".to_string()),
        Some("md") => Some("text/markdown".to_string()),
        Some("json") => Some("application/json".to_string()),
        _ => None,
    }
}

pub(super) fn extension_for_content_type(content_type: Option<&str>) -> Option<String> {
    let content_type = content_type?.split(';').next()?.trim().to_ascii_lowercase();
    match content_type.as_str() {
        "text/html" => Some("html".to_string()),
        "text/plain" => Some("txt".to_string()),
        "text/markdown" => Some("md".to_string()),
        "application/json" => Some("json".to_string()),
        "application/xml" | "text/xml" => Some("xml".to_string()),
        "image/png" => Some("png".to_string()),
        "image/jpeg" => Some("jpg".to_string()),
        "image/gif" => Some("gif".to_string()),
        "image/webp" => Some("webp".to_string()),
        "application/pdf" => Some("pdf".to_string()),
        _ => None,
    }
}

pub(super) fn filename_extension(path_or_url: &str) -> Option<String> {
    let path = path_or_url.split(['?', '#']).next().unwrap_or(path_or_url);
    path.rsplit('/').next().and_then(|name| {
        name.rsplit_once('.').and_then(|(_, extension)| {
            let normalized = extension.to_ascii_lowercase();
            (!normalized.is_empty()
                && normalized.len() <= 12
                && normalized
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric()))
            .then_some(normalized)
        })
    })
}
