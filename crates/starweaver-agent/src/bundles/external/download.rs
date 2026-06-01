use serde_json::Value;
use starweaver_context::AgentContext;
use starweaver_environment::DynEnvironmentProvider;
use starweaver_tools::{ToolContext, ToolError, ToolResult};
use uuid::Uuid;

use super::{
    args::DownloadArgs,
    http::{
        extension_for_content_type, fetch_http_resource, filename_extension, is_text_like,
        looks_textual, MAX_DOWNLOAD_BYTES,
    },
};
use crate::bundles::{helpers::tool_execution_error, EnvironmentHandle};

pub(super) async fn download(
    context: ToolContext,
    arguments: DownloadArgs,
) -> Result<ToolResult, ToolError> {
    let provider = environment_provider(&context, "download")?;
    let mut records = Vec::new();
    for url in arguments.urls {
        records.push(download_one(&context, provider.clone(), &url, &arguments.save_dir).await?);
    }
    let success = records
        .iter()
        .all(|record| record.get("success").and_then(Value::as_bool) == Some(true));
    Ok(ToolResult::new(serde_json::json!({
        "success": success,
        "save_dir": arguments.save_dir,
        "downloads": records,
    })))
}

async fn download_one(
    context: &ToolContext,
    provider: DynEnvironmentProvider,
    url: &str,
    save_dir: &str,
) -> Result<Value, ToolError> {
    let resource = fetch_http_resource(
        context,
        "download",
        url,
        reqwest::Method::GET,
        MAX_DOWNLOAD_BYTES,
    )
    .await?;
    let body = resource.body.unwrap_or_default();
    let text = match std::str::from_utf8(&body) {
        Ok(text) if is_text_like(resource.content_type.as_deref()) || looks_textual(text) => text,
        Ok(text) if resource.content_type.is_none() => text,
        _ => {
            return Ok(serde_json::json!({
                "success": false,
                "url": url,
                "final_url": resource.final_url,
                "content_type": resource.content_type,
                "byte_size": body.len(),
                "error": "binary downloads require a binary resource EnvironmentProvider extension",
            }));
        }
    };
    let extension = filename_extension(url)
        .or_else(|| extension_for_content_type(resource.content_type.as_deref()))
        .unwrap_or_else(|| "txt".to_string());
    let filename = format!("download-{}.{}", Uuid::new_v4(), extension);
    let path = safe_download_path(save_dir, &filename)?;
    provider
        .write_text(&path, text)
        .await
        .map_err(|error| tool_execution_error("download", error))?;
    Ok(serde_json::json!({
        "success": true,
        "url": url,
        "final_url": resource.final_url,
        "path": path,
        "content_type": resource.content_type,
        "byte_size": body.len(),
        "text": true,
    }))
}

fn safe_download_path(save_dir: &str, filename: &str) -> Result<String, ToolError> {
    let normalized_dir = save_dir.replace('\\', "/");
    let trimmed = normalized_dir.trim_matches('/');
    if trimmed
        .split('/')
        .any(|segment| matches!(segment, ".." | ".") || segment.is_empty() && !trimmed.is_empty())
    {
        return Err(tool_execution_error("download", "invalid save_dir"));
    }
    if trimmed.is_empty() {
        Ok(filename.to_string())
    } else {
        Ok(format!("{trimmed}/{filename}"))
    }
}

fn environment_provider(
    context: &ToolContext,
    tool: &str,
) -> Result<DynEnvironmentProvider, ToolError> {
    let agent_context = context.dependency::<AgentContext>().ok_or_else(|| {
        tool_execution_error(tool, "AgentContext dependency is missing from ToolContext")
    })?;
    let environment = agent_context
        .dependencies
        .get::<EnvironmentHandle>()
        .ok_or_else(|| {
            tool_execution_error(tool, "EnvironmentHandle is missing from AgentContext")
        })?;
    Ok(environment.provider())
}
