//! Tool return media request helpers.

use starweaver_model::{
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, ModelRequestPart,
    ToolReturnPart,
};

pub(in crate::agent) fn tool_return_media_prompt(
    tool_return: &ToolReturnPart,
) -> Option<ModelRequestPart> {
    let value = tool_return
        .private_metadata
        .get("starweaver_tool_return_content_parts")?
        .clone();
    let mut content = Vec::new();
    let prompt = tool_return
        .private_metadata
        .get("starweaver_tool_return_prompt")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map_or_else(
            || {
                format!(
                    "Tool {} returned provider-native media content.",
                    tool_return.name
                )
            },
            str::to_string,
        );
    content.push(ContentPart::Text { text: prompt });
    let mut media_parts = serde_json::from_value::<Vec<ContentPart>>(value).ok()?;
    if media_parts.is_empty() {
        return None;
    }
    content.append(&mut media_parts);
    let mut metadata = serde_json::Map::new();
    metadata.insert(
        CONTEXT_ORIGIN_METADATA.to_string(),
        serde_json::json!(CONTEXT_ORIGIN_TOOL_RETURN_MEDIA),
    );
    metadata.insert(
        "tool_call_id".to_string(),
        serde_json::json!(tool_return.tool_call_id.clone()),
    );
    metadata.insert(
        "tool_name".to_string(),
        serde_json::json!(tool_return.name.clone()),
    );
    Some(ModelRequestPart::UserPrompt {
        content,
        name: None,
        metadata,
    })
}
