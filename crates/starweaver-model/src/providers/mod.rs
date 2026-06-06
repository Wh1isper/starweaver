//! Wire mappers for the first supported provider protocol families.

pub mod anthropic;
pub mod bedrock;
pub mod client;
pub mod gemini;
pub mod openai_chat;
pub mod openai_responses;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Map, Value};

use crate::{
    message::{ContentPart, FinishReason, ModelMessage, ModelRequestPart, ToolArguments},
    settings::ToolChoice,
    transport::MaxTokensParameter,
    ModelSettings,
};

fn text_from_content(content: &[ContentPart]) -> String {
    content
        .iter()
        .filter_map(|part| match part {
            ContentPart::Text { text } => Some(text.as_str()),
            ContentPart::ImageUrl { .. }
            | ContentPart::FileUrl { .. }
            | ContentPart::Binary { .. }
            | ContentPart::ResourceRef { .. }
            | ContentPart::DataUrl { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

pub(crate) fn openai_chat_content(content: &[ContentPart]) -> Value {
    if content.len() == 1 {
        if let ContentPart::Text { text } = &content[0] {
            return json!(text);
        }
    }
    Value::Array(
        content
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => json!({"type": "text", "text": text}),
                ContentPart::ImageUrl { url } => {
                    json!({"type": "image_url", "image_url": {"url": url}})
                }
                ContentPart::FileUrl { url, media_type } => json!({
                    "type": "file",
                    "file": {"file_data": url, "media_type": media_type}
                }),
                ContentPart::Binary { data, media_type } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": data_url(media_type, data)}})
                    } else {
                        json!({"type": "file", "file": {"file_data": data_url(media_type, data), "media_type": media_type}})
                    }
                }
                ContentPart::ResourceRef { uri, media_type, .. } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": uri}})
                    } else {
                        json!({"type": "file", "file": {"file_data": uri, "media_type": media_type}})
                    }
                }
                ContentPart::DataUrl { data_url, media_type } => {
                    if media_type.starts_with("image/") {
                        json!({"type": "image_url", "image_url": {"url": data_url}})
                    } else {
                        json!({"type": "file", "file": {"file_data": data_url, "media_type": media_type}})
                    }
                }
            })
            .collect(),
    )
}

pub(crate) fn openai_responses_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"type": "input_text", "text": text}),
            ContentPart::ImageUrl { url } => json!({"type": "input_image", "image_url": url}),
            ContentPart::FileUrl { url, media_type } => json!({
                "type": "input_file",
                "file_url": url,
                "media_type": media_type
            }),
            ContentPart::Binary { data, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": data_url(media_type, data)})
                } else {
                    json!({"type": "input_file", "file_url": data_url(media_type, data), "media_type": media_type})
                }
            }
            ContentPart::ResourceRef { uri, media_type, .. } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": uri})
                } else {
                    json!({"type": "input_file", "file_url": uri, "media_type": media_type})
                }
            }
            ContentPart::DataUrl { data_url, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"type": "input_image", "image_url": data_url})
                } else {
                    json!({"type": "input_file", "file_url": data_url, "media_type": media_type})
                }
            }
        })
        .collect()
}

pub(crate) fn gemini_parts_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({
                "fileData": {"fileUri": url, "mimeType": "image/*"}
            }),
            ContentPart::FileUrl { url, media_type } => json!({
                "fileData": {"fileUri": url, "mimeType": media_type}
            }),
            ContentPart::Binary { data, media_type } => json!({
                "inlineData": {"data": base64_payload(data), "mimeType": media_type}
            }),
            ContentPart::ResourceRef {
                uri, media_type, ..
            } => json!({
                "fileData": {"fileUri": uri, "mimeType": media_type}
            }),
            ContentPart::DataUrl {
                data_url,
                media_type,
            } => json!({
                "fileData": {"fileUri": data_url, "mimeType": media_type}
            }),
        })
        .collect()
}

pub(crate) fn bedrock_content_from_content(content: &[ContentPart]) -> Vec<Value> {
    content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => json!({"text": text}),
            ContentPart::ImageUrl { url } => json!({"image": {"source": {"bytes": url}}}),
            ContentPart::FileUrl { url, media_type } => json!({
                "document": {
                    "format": media_type,
                    "source": {"bytes": url},
                }
            }),
            ContentPart::Binary { data, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"image": {"format": bedrock_media_format(media_type), "source": {"bytes": base64_payload(data)}}})
                } else {
                    json!({"document": {"format": bedrock_media_format(media_type), "source": {"bytes": base64_payload(data)}}})
                }
            }
            ContentPart::ResourceRef { uri, media_type, .. } | ContentPart::DataUrl { data_url: uri, media_type } => {
                if media_type.starts_with("image/") {
                    json!({"image": {"format": bedrock_media_format(media_type), "source": {"bytes": uri}}})
                } else {
                    json!({"document": {"format": bedrock_media_format(media_type), "source": {"bytes": uri}}})
                }
            }
        })
        .collect()
}

fn data_url(media_type: &str, data: &[u8]) -> String {
    format!("data:{media_type};base64,{}", base64_payload(data))
}

fn base64_payload(data: &[u8]) -> String {
    STANDARD.encode(data)
}

fn bedrock_media_format(media_type: &str) -> &str {
    media_type
        .strip_prefix("image/")
        .or_else(|| media_type.strip_prefix("application/"))
        .unwrap_or(media_type)
}

fn collect_system_and_non_system(messages: &[ModelMessage]) -> (Vec<String>, Vec<&ModelMessage>) {
    let mut system = Vec::new();
    let mut rest = Vec::new();

    for message in messages {
        match message {
            ModelMessage::Request(request) => {
                let mut has_non_system = false;
                for part in &request.parts {
                    match part {
                        ModelRequestPart::SystemPrompt { text, .. }
                        | ModelRequestPart::Instruction { text, .. } => system.push(text.clone()),
                        _ => has_non_system = true,
                    }
                }
                if has_non_system {
                    rest.push(message);
                }
            }
            ModelMessage::Response(_) => rest.push(message),
        }
    }

    (system, rest)
}

fn usage_from_openai(value: &Value) -> starweaver_core::Usage {
    let usage = value.get("usage");
    starweaver_core::Usage {
        requests: 1,
        input_tokens: usage
            .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        output_tokens: usage
            .and_then(|u| {
                u.get("completion_tokens")
                    .or_else(|| u.get("output_tokens"))
            })
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        total_tokens: usage
            .and_then(|u| u.get("total_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        tool_calls: 0,
    }
}

fn usage_from_named(value: &Value, input: &str, output: &str) -> starweaver_core::Usage {
    let usage = value.get("usage").or_else(|| value.get("usageMetadata"));
    let input_tokens = usage
        .and_then(|u| u.get(input))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output_tokens = usage
        .and_then(|u| u.get(output))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    starweaver_core::Usage {
        requests: 1,
        input_tokens,
        output_tokens,
        total_tokens: usage
            .and_then(|u| u.get("totalTokens").or_else(|| u.get("total_tokens")))
            .and_then(Value::as_u64)
            .unwrap_or(input_tokens + output_tokens),
        tool_calls: 0,
    }
}

fn finish_reason_openai(reason: &str) -> FinishReason {
    match reason {
        "stop" | "completed" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Unknown,
    }
}

pub(crate) fn provider_tool_parameters(parameters: &Value) -> Value {
    let mut schema = parameters.clone();
    remove_schema_meta(&mut schema);
    schema
}

pub(crate) fn insert_optional_description(
    object: &mut Map<String, Value>,
    description: Option<&String>,
) {
    if let Some(description) = description
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        object.insert("description".to_string(), json!(description));
    }
}

fn remove_schema_meta(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("$schema");
            for nested in object.values_mut() {
                remove_schema_meta(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                remove_schema_meta(item);
            }
        }
        _ => {}
    }
}

fn apply_common_settings(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
) {
    apply_common_settings_with_max_tokens(target, settings, MaxTokensParameter::MaxTokens);
}

pub(crate) fn apply_common_settings_with_max_tokens(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_parameter: MaxTokensParameter,
) {
    let max_tokens_key = match max_tokens_parameter {
        MaxTokensParameter::Default | MaxTokensParameter::MaxTokens => Some("max_tokens"),
        MaxTokensParameter::MaxOutputTokens => Some("max_output_tokens"),
        MaxTokensParameter::Omit => None,
    };
    apply_common_settings_inner(target, settings, max_tokens_key);
}

fn apply_common_settings_inner(
    target: &mut serde_json::Map<String, Value>,
    settings: Option<&ModelSettings>,
    max_tokens_key: Option<&str>,
) {
    if let Some(settings) = settings {
        if let (Some(key), Some(max_tokens)) = (max_tokens_key, settings.max_tokens) {
            target.insert(key.to_string(), json!(max_tokens));
        }
        if let Some(temperature) = settings.temperature {
            target.insert("temperature".to_string(), json!(temperature));
        }
        if let Some(top_p) = settings.top_p {
            target.insert("top_p".to_string(), json!(top_p));
        }
        if !settings.stop_sequences.is_empty() {
            target.insert("stop".to_string(), json!(settings.stop_sequences));
        }
        if let Some(parallel_tool_calls) = settings.parallel_tool_calls {
            target.insert(
                "parallel_tool_calls".to_string(),
                json!(parallel_tool_calls),
            );
        }
        if let Some(thinking) = &settings.thinking {
            target.insert("reasoning_effort".to_string(), json!(thinking.effort));
        }
        if let Some(service_tier) = &settings.service_tier {
            target.insert("service_tier".to_string(), json!(service_tier));
        }
        if let Some(options) = settings
            .provider_options
            .as_ref()
            .and_then(Value::as_object)
        {
            for (key, value) in options {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

pub(crate) fn openai_chat_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "function": {"name": name}
        }),
    }
}

pub(crate) fn openai_responses_tool_choice(choice: &ToolChoice) -> Value {
    match choice {
        ToolChoice::Auto => json!("auto"),
        ToolChoice::None => json!("none"),
        ToolChoice::Required => json!("required"),
        ToolChoice::Tool { name } => json!({
            "type": "function",
            "name": name,
        }),
    }
}

fn parse_tool_call_arguments(value: &Value) -> ToolArguments {
    ToolArguments::from_provider_value(value)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::message::{ModelRequest, ModelResponse, ToolReturnPart};
    use crate::{ServiceTier, ThinkingSettings};

    fn mixed_content() -> Vec<ContentPart> {
        vec![
            ContentPart::Text {
                text: "hello".to_string(),
            },
            ContentPart::ImageUrl {
                url: "https://example.test/image.png".to_string(),
            },
            ContentPart::FileUrl {
                url: "https://example.test/file.pdf".to_string(),
                media_type: "application/pdf".to_string(),
            },
            ContentPart::Binary {
                data: vec![1, 2, 3],
                media_type: "image/png".to_string(),
            },
            ContentPart::Binary {
                data: vec![4, 5, 6],
                media_type: "application/json".to_string(),
            },
            ContentPart::ResourceRef {
                uri: "resource://image/1".to_string(),
                media_type: "image/jpeg".to_string(),
                resource_type: "image".to_string(),
                metadata: Map::new(),
            },
            ContentPart::ResourceRef {
                uri: "resource://doc/1".to_string(),
                media_type: "application/pdf".to_string(),
                resource_type: "document".to_string(),
                metadata: Map::new(),
            },
            ContentPart::DataUrl {
                data_url: "data:image/png;base64,abc=".to_string(),
                media_type: "image/png".to_string(),
            },
            ContentPart::DataUrl {
                data_url: "data:application/pdf;base64,abc=".to_string(),
                media_type: "application/pdf".to_string(),
            },
        ]
    }

    #[test]
    fn content_mappers_cover_text_binary_resource_and_data_url_variants() {
        assert_eq!(text_from_content(&mixed_content()), "hello");
        assert_eq!(
            openai_chat_content(&[ContentPart::Text {
                text: "solo".to_string()
            }]),
            json!("solo")
        );

        let chat = openai_chat_content(&mixed_content());
        assert_eq!(chat[0]["type"], "text");
        assert_eq!(chat[1]["type"], "image_url");
        assert_eq!(chat[2]["type"], "file");
        assert!(chat[3]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(chat[4]["file"]["file_data"]
            .as_str()
            .unwrap()
            .starts_with("data:application/json;base64,"));
        assert_eq!(chat[5]["image_url"]["url"], "resource://image/1");
        assert_eq!(chat[6]["file"]["file_data"], "resource://doc/1");
        assert_eq!(chat[7]["image_url"]["url"], "data:image/png;base64,abc=");
        assert_eq!(
            chat[8]["file"]["file_data"],
            "data:application/pdf;base64,abc="
        );

        let responses = openai_responses_content(&mixed_content());
        assert_eq!(responses[0]["type"], "input_text");
        assert_eq!(responses[1]["type"], "input_image");
        assert_eq!(responses[2]["type"], "input_file");
        assert!(responses[3]["image_url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
        assert!(responses[4]["file_url"]
            .as_str()
            .unwrap()
            .starts_with("data:application/json;base64,"));

        let gemini = gemini_parts_from_content(&mixed_content());
        assert_eq!(gemini[0]["text"], "hello");
        assert_eq!(gemini[1]["fileData"]["mimeType"], "image/*");
        assert_eq!(gemini[2]["fileData"]["mimeType"], "application/pdf");
        assert_eq!(gemini[3]["inlineData"]["data"], "AQID");
        assert_eq!(gemini[5]["fileData"]["fileUri"], "resource://image/1");

        let bedrock = bedrock_content_from_content(&mixed_content());
        assert_eq!(bedrock[0]["text"], "hello");
        assert_eq!(
            bedrock[1]["image"]["source"]["bytes"],
            "https://example.test/image.png"
        );
        assert_eq!(bedrock[2]["document"]["format"], "application/pdf");
        assert_eq!(bedrock[3]["image"]["format"], "png");
        assert_eq!(bedrock[4]["document"]["format"], "json");
        assert_eq!(bedrock[5]["image"]["source"]["bytes"], "resource://image/1");
    }

    #[test]
    fn provider_schema_helpers_strip_meta_and_descriptions() {
        let mut schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "nested": {"$schema": "nested", "type": "string"},
                "items": [{"$schema": "array", "type": "number"}]
            }
        });
        schema = provider_tool_parameters(&schema);
        assert!(schema.get("$schema").is_none());
        assert!(schema["properties"]["nested"].get("$schema").is_none());
        assert!(schema["properties"]["items"][0].get("$schema").is_none());

        let mut object = Map::new();
        insert_optional_description(&mut object, Some(&" useful ".to_string()));
        insert_optional_description(&mut object, Some(&"   ".to_string()));
        insert_optional_description(&mut object, None);
        assert_eq!(object["description"], " useful ");
    }

    #[test]
    fn provider_settings_helpers_apply_tokens_sampling_and_options() {
        let settings = ModelSettings {
            max_tokens: Some(128),
            temperature: Some(0.2),
            top_p: Some(0.9),
            stop_sequences: vec!["stop".to_string()],
            parallel_tool_calls: Some(true),
            thinking: Some(ThinkingSettings {
                effort: "high".to_string(),
                budget_tokens: None,
                mode: None,
                include_thoughts: None,
                summary: None,
            }),
            service_tier: Some(ServiceTier::Priority),
            provider_options: Some(json!({"store": false})),
            ..ModelSettings::default()
        };
        let mut target = Map::new();
        apply_common_settings(&mut target, Some(&settings));
        assert_eq!(target["max_tokens"], 128);
        assert_eq!(target["temperature"], 0.2);
        assert_eq!(target["top_p"], 0.9);
        assert_eq!(target["stop"], json!(["stop"]));
        assert_eq!(target["parallel_tool_calls"], true);
        assert_eq!(target["reasoning_effort"], "high");
        assert_eq!(target["service_tier"], "priority");
        assert_eq!(target["store"], false);

        let mut output_tokens_target = Map::new();
        apply_common_settings_with_max_tokens(
            &mut output_tokens_target,
            Some(&settings),
            MaxTokensParameter::MaxOutputTokens,
        );
        assert_eq!(output_tokens_target["max_output_tokens"], 128);
        let mut omitted = Map::new();
        apply_common_settings_with_max_tokens(
            &mut omitted,
            Some(&settings),
            MaxTokensParameter::Omit,
        );
        assert!(omitted.get("max_tokens").is_none());
    }

    #[test]
    fn provider_tool_choice_usage_finish_and_arguments_are_mapped() {
        assert_eq!(openai_chat_tool_choice(&ToolChoice::Auto), json!("auto"));
        assert_eq!(openai_chat_tool_choice(&ToolChoice::None), json!("none"));
        assert_eq!(
            openai_chat_tool_choice(&ToolChoice::Required),
            json!("required")
        );
        assert_eq!(
            openai_chat_tool_choice(&ToolChoice::Tool {
                name: "lookup".to_string()
            })["function"]["name"],
            "lookup"
        );
        assert_eq!(
            openai_responses_tool_choice(&ToolChoice::Auto),
            json!("auto")
        );
        assert_eq!(
            openai_responses_tool_choice(&ToolChoice::None),
            json!("none")
        );
        assert_eq!(
            openai_responses_tool_choice(&ToolChoice::Required),
            json!("required")
        );
        assert_eq!(
            openai_responses_tool_choice(&ToolChoice::Tool {
                name: "lookup".to_string()
            })["name"],
            "lookup"
        );

        let openai_usage = usage_from_openai(
            &json!({"usage": {"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 3}}),
        );
        assert_eq!(openai_usage.input_tokens, 1);
        assert_eq!(openai_usage.output_tokens, 2);
        let named_usage = usage_from_named(
            &json!({"usageMetadata": {"promptTokenCount": 4, "candidatesTokenCount": 5}}),
            "promptTokenCount",
            "candidatesTokenCount",
        );
        assert_eq!(named_usage.total_tokens, 9);

        assert_eq!(finish_reason_openai("stop"), FinishReason::Stop);
        assert_eq!(finish_reason_openai("completed"), FinishReason::Stop);
        assert_eq!(finish_reason_openai("length"), FinishReason::Length);
        assert_eq!(finish_reason_openai("tool_calls"), FinishReason::ToolCalls);
        assert_eq!(
            finish_reason_openai("content_filter"),
            FinishReason::ContentFilter
        );
        assert_eq!(finish_reason_openai("other"), FinishReason::Unknown);
        assert_eq!(
            parse_tool_call_arguments(&json!("{\"ok\":true}")).execution_value()["ok"],
            true
        );
        assert_eq!(
            parse_tool_call_arguments(&json!("not-json")).execution_value(),
            json!("not-json")
        );
        assert!(parse_tool_call_arguments(&json!("not-json"))
            .invalid_error()
            .is_some());
        assert_eq!(
            parse_tool_call_arguments(&json!({"already": true})).execution_value()["already"],
            true
        );
    }

    #[test]
    fn collect_system_prompts_preserves_non_system_messages() {
        let request = ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "system".to_string(),
                    metadata: Map::new(),
                },
                ModelRequestPart::Instruction {
                    text: "instruction".to_string(),
                    metadata: Map::new(),
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "user".to_string(),
                    }],
                    name: None,
                    metadata: Map::new(),
                },
            ],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        });
        let system_only = ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::SystemPrompt {
                text: "only-system".to_string(),
                metadata: Map::new(),
            }],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        });
        let response = ModelMessage::Response(ModelResponse::text("assistant"));
        let tool_return = ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart {
                tool_call_id: "call_1".to_string(),
                name: "tool".to_string(),
                content: json!({"ok": true}),
                is_error: false,
                metadata: Map::new(),
            })],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        });

        let messages = vec![request, system_only, response, tool_return];
        let (system, rest) = collect_system_and_non_system(&messages);
        assert_eq!(system, ["system", "instruction", "only-system"]);
        assert_eq!(rest.len(), 3);
    }
}
