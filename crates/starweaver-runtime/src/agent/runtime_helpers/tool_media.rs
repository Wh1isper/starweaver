//! Tool return media request helpers.

use starweaver_model::{
    CONTEXT_ORIGIN_METADATA, CONTEXT_ORIGIN_TOOL_RETURN_MEDIA, ContentPart, ModelRequestPart,
    ToolReturnPart,
};

pub(in crate::agent) const TOOL_RETURN_CONTENT_PARTS_KEY: &str =
    "starweaver_tool_return_content_parts";
pub(in crate::agent) const TOOL_RETURN_MEDIA_PROMPT_KEY: &str = "starweaver_tool_return_prompt";
pub(in crate::agent) const GEOMETRY_BOUND_MEDIA_KEY: &str =
    "starweaver_geometry_bound_immutable_media";

pub(in crate::agent) fn tool_return_media_prompt(
    tool_return: &ToolReturnPart,
) -> Option<ModelRequestPart> {
    let value = tool_return
        .private_metadata
        .get(TOOL_RETURN_CONTENT_PARTS_KEY)?
        .clone();
    let mut content = Vec::new();
    let prompt = tool_return
        .private_metadata
        .get(TOOL_RETURN_MEDIA_PROMPT_KEY)
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
    if tool_return
        .private_metadata
        .get(GEOMETRY_BOUND_MEDIA_KEY)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        metadata.insert(
            GEOMETRY_BOUND_MEDIA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );
    }
    Some(ModelRequestPart::UserPrompt {
        content,
        name: None,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use starweaver_model::ToolReturnPart;

    use super::{GEOMETRY_BOUND_MEDIA_KEY, tool_return_media_prompt};

    #[test]
    fn geometry_bound_marker_is_propagated_to_media_prompt() {
        let mut private_metadata = serde_json::Map::from_iter([
            (
                "starweaver_tool_return_content_parts".to_owned(),
                json!([{
                    "kind": "data_url",
                    "data_url": "data:image/png;base64,AA==",
                    "media_type": "image/png"
                }]),
            ),
            (
                "starweaver_geometry_bound_immutable_media".to_owned(),
                json!(true),
            ),
        ]);
        private_metadata.insert(
            "starweaver_tool_return_prompt".to_owned(),
            json!("geometry-bound screenshot"),
        );
        let tool_return = ToolReturnPart::new("call-1", "computer_observe", json!({}))
            .with_private_metadata(private_metadata);

        let Some(starweaver_model::ModelRequestPart::UserPrompt { metadata, .. }) =
            tool_return_media_prompt(&tool_return)
        else {
            panic!("tool media should produce a user prompt");
        };
        assert_eq!(
            metadata
                .get(GEOMETRY_BOUND_MEDIA_KEY)
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }
}
