use serde_json::Value;

use crate::{
    adapter::ModelRequestParameters,
    profile::{ModelProfile, ProtocolFamily},
    request::OutputMode,
};

#[allow(clippy::too_many_lines)]
pub(super) fn apply_output_schema(
    body: &mut Value,
    profile: &ModelProfile,
    params: &ModelRequestParameters,
) {
    let Some(output_mode) = params.output_mode else {
        return;
    };
    if !matches!(
        output_mode,
        OutputMode::NativeJsonSchema | OutputMode::NativeJsonObject
    ) {
        return;
    }
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let schema = params.output_schema.as_ref();
    match profile.protocol {
        ProtocolFamily::OpenAiChatCompletions => match output_mode {
            OutputMode::NativeJsonSchema => {
                let Some(schema) = schema else {
                    return;
                };
                object.insert(
                    "response_format".to_string(),
                    serde_json::json!({
                        "type": "json_schema",
                        "json_schema": schema,
                    }),
                );
            }
            OutputMode::NativeJsonObject => {
                object.insert(
                    "response_format".to_string(),
                    serde_json::json!({"type": "json_object"}),
                );
            }
            OutputMode::Text | OutputMode::Tool | OutputMode::Prompted => {}
        },
        ProtocolFamily::OpenAiResponses => match output_mode {
            OutputMode::NativeJsonSchema => {
                let Some(schema) = schema else {
                    return;
                };
                let text = object
                    .entry("text".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(text) = text.as_object_mut() {
                    text.insert(
                        "format".to_string(),
                        serde_json::json!({
                            "type": "json_schema",
                            "name": schema.get("name").and_then(Value::as_str).unwrap_or("output"),
                            "schema": schema.get("schema").cloned().unwrap_or_else(|| schema.clone()),
                            "strict": schema.get("strict").and_then(Value::as_bool).unwrap_or(true),
                        }),
                    );
                }
            }
            OutputMode::NativeJsonObject => {
                let text = object
                    .entry("text".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(text) = text.as_object_mut() {
                    text.insert(
                        "format".to_string(),
                        serde_json::json!({"type": "json_object"}),
                    );
                }
            }
            OutputMode::Text | OutputMode::Tool | OutputMode::Prompted => {}
        },
        ProtocolFamily::GeminiGenerateContent => {
            let generation_config = object
                .entry("generationConfig".to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Some(generation_config) = generation_config.as_object_mut() {
                generation_config.insert(
                    "responseMimeType".to_string(),
                    Value::String("application/json".to_string()),
                );
                if let (OutputMode::NativeJsonSchema, Some(schema)) = (output_mode, schema) {
                    generation_config.insert(
                        "responseSchema".to_string(),
                        schema
                            .get("schema")
                            .cloned()
                            .unwrap_or_else(|| schema.clone()),
                    );
                }
            }
        }
        ProtocolFamily::AnthropicMessages => {
            if let (OutputMode::NativeJsonSchema, Some(schema)) = (output_mode, schema) {
                let output_config = object
                    .entry("output_config".to_string())
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                if let Some(output_config) = output_config.as_object_mut() {
                    output_config.insert(
                        "format".to_string(),
                        serde_json::json!({
                            "type": "json_schema",
                            "schema": schema.get("schema").cloned().unwrap_or_else(|| schema.clone()),
                        }),
                    );
                }
            }
        }
        ProtocolFamily::BedrockConverse => {}
    }
}
