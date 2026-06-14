use serde_json::{json, Map};

use super::*;
use crate::message::{
    ContentPart, FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse,
    ToolReturnPart,
};
use crate::transport::MaxTokensParameter;
use crate::{ModelSettings, ServiceTier, ThinkingSettings, ToolChoice};

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
    schema = provider_tool_schema_without_meta(&schema);
    assert!(schema.get("$schema").is_none());
    assert!(schema["properties"]["nested"].get("$schema").is_none());
    assert!(schema["properties"]["items"][0].get("$schema").is_none());

    let mut object = Map::new();
    insert_nonempty_description(&mut object, Some(&" useful ".to_string()));
    insert_nonempty_description(&mut object, Some(&"   ".to_string()));
    insert_nonempty_description(&mut object, None);
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
    apply_common_settings_with_max_tokens(&mut omitted, Some(&settings), MaxTokensParameter::Omit);
    assert!(omitted.get("max_tokens").is_none());
}

#[test]
#[allow(clippy::too_many_lines)]
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

    let openai_usage = usage_from_openai(&json!({"usage": {
        "prompt_tokens": 1,
        "completion_tokens": 2,
        "total_tokens": 3,
        "prompt_tokens_details": {"cached_tokens": 4}
    }}));
    assert_eq!(openai_usage.input_tokens, 1);
    assert_eq!(openai_usage.cache_read_tokens, 4);
    assert_eq!(openai_usage.output_tokens, 2);
    assert_eq!(openai_usage.total_tokens, 3);
    let openai_usage_without_total = usage_from_openai(&json!({"usage": {
        "prompt_tokens": 3,
        "completion_tokens": 4
    }}));
    assert_eq!(openai_usage_without_total.total_tokens, 7);
    let responses_usage = usage_from_openai(&json!({"usage": {
        "input_tokens": 10,
        "output_tokens": 3,
        "total_tokens": 13,
        "input_tokens_details": {"cached_tokens": 6}
    }}));
    assert_eq!(responses_usage.cache_read_tokens, 6);
    let named_usage = usage_from_named(
        &json!({"usageMetadata": {
            "promptTokenCount": 4,
            "candidatesTokenCount": 5,
            "cachedContentTokenCount": 3
        }}),
        "promptTokenCount",
        "candidatesTokenCount",
    );
    assert_eq!(named_usage.cache_read_tokens, 3);
    assert_eq!(named_usage.total_tokens, 9);
    let gemini_usage = usage_from_named_with_output_extras(
        &json!({"usageMetadata": {
            "promptTokenCount": 4,
            "candidatesTokenCount": 5,
            "cachedContentTokenCount": 3,
            "thoughtsTokenCount": 2
        }}),
        "promptTokenCount",
        "candidatesTokenCount",
        &["thoughtsTokenCount"],
    );
    assert_eq!(gemini_usage.cache_read_tokens, 3);
    assert_eq!(gemini_usage.output_tokens, 7);
    assert_eq!(gemini_usage.total_tokens, 11);
    let anthropic_usage = usage_from_named_including_cache_input(
        &json!({"usage": {
            "input_tokens": 7,
            "output_tokens": 8,
            "cache_creation_input_tokens": 9,
            "cache_read_input_tokens": 10
        }}),
        "input_tokens",
        "output_tokens",
    );
    assert_eq!(anthropic_usage.input_tokens, 26);
    assert_eq!(anthropic_usage.cache_write_tokens, 9);
    assert_eq!(anthropic_usage.cache_read_tokens, 10);
    assert_eq!(anthropic_usage.total_tokens, 34);

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
    let mut dynamic_metadata = Map::new();
    dynamic_metadata.insert(
        "starweaver_instruction_origin".to_string(),
        json!("environment_context"),
    );
    let request = ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "system".to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::Instruction {
                text: "instruction".to_string(),
                metadata: dynamic_metadata,
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
        parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
            "call_1",
            "tool",
            json!({"ok": true}),
        ))],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: Map::new(),
    });

    let messages = vec![request, system_only, response, tool_return];
    let (system_parts, _) = collect_system_parts_and_non_system(&messages);
    assert!(!system_parts[0].dynamic);
    assert!(system_parts[1].dynamic);
    assert!(!system_parts[2].dynamic);
    let (system, rest) = collect_system_and_non_system(&messages);
    assert_eq!(system, ["system", "instruction", "only-system"]);
    assert_eq!(rest.len(), 3);
}
