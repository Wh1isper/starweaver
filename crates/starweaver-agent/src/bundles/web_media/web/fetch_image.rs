use serde_json::Map;
use starweaver_context::AgentContext;
use starweaver_tools::{ToolContext, ToolError, ToolResult};

use crate::{
    bundles::helpers::tool_feedback,
    media_compression::{compress_image_to_model_limit, data_url, raw_budget_for_encoded_limit},
};

pub(super) fn fetch_image_result(
    context: &ToolContext,
    requested_url: &str,
    resource: &super::super::http::HttpResource,
    mut body: Vec<u8>,
) -> Result<ToolResult, ToolError> {
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
                        return Err(tool_feedback(
                            "fetch",
                            format!(
                                "Fetched image could not be compressed below the {max_image_bytes} byte API limit after accounting for base64 encoding. Download the image and resize or convert it to a smaller format before retrying."
                            ),
                        ));
                    }
                    body = compressed.data;
                    media_type = compressed.media_type;
                    compressed_for_model = compressed.compressed;
                }
                Err(error) => {
                    return Err(tool_feedback(
                        "fetch",
                        format!(
                            "Fetched image could not be compressed for inline model input: {error}. Download the image and resize or convert it to a supported smaller format before retrying."
                        ),
                    ));
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
    Ok(ToolResult::new(serde_json::json!({
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
    )))
}
