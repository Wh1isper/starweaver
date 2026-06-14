use serde_json::Value;

use crate::{
    adapter::ModelRequestParameters,
    profile::{ModelProfile, ProtocolFamily},
    request::OutputMode,
};

pub(super) fn apply_output_schema(
    body: &mut Value,
    profile: &ModelProfile,
    params: &ModelRequestParameters,
) {
    if !matches!(params.output_mode, Some(OutputMode::NativeJsonSchema)) {
        return;
    }
    let Some(schema) = params.output_schema.as_ref() else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    match profile.protocol {
        ProtocolFamily::OpenAiChatCompletions => {
            object.insert(
                "response_format".to_string(),
                serde_json::json!({
                    "type": "json_schema",
                    "json_schema": schema,
                }),
            );
        }
        ProtocolFamily::OpenAiResponses => {
            object.insert(
                "text".to_string(),
                serde_json::json!({
                    "format": {
                        "type": "json_schema",
                        "name": schema.get("name").and_then(Value::as_str).unwrap_or("output"),
                        "schema": schema.get("schema").cloned().unwrap_or_else(|| schema.clone()),
                        "strict": schema.get("strict").and_then(Value::as_bool).unwrap_or(true),
                    }
                }),
            );
        }
        ProtocolFamily::GeminiGenerateContent => {
            let generation_config = object
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(generation_config) = generation_config.as_object_mut() {
                generation_config.insert(
                    "responseMimeType".to_string(),
                    Value::String("application/json".to_string()),
                );
                generation_config.insert(
                    "responseSchema".to_string(),
                    schema
                        .get("schema")
                        .cloned()
                        .unwrap_or_else(|| schema.clone()),
                );
            }
        }
        ProtocolFamily::AnthropicMessages | ProtocolFamily::BedrockConverse => {}
    }
}
