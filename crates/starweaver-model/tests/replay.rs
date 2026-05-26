#![allow(missing_docs, clippy::unwrap_used)]

use serde_json::{json, Value};
use starweaver_model::{
    adapter::{NativeToolDefinition, ToolDefinition},
    message::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart},
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    ModelResponsePart, ModelSettings,
};

fn canonical_history() -> Vec<ModelMessage> {
    vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "You are concise.".to_string(),
                metadata: serde_json::Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "What is 2+2?".to_string(),
                }],
                name: None,
                metadata: serde_json::Map::new(),
            },
        ],
        timestamp: None,
        instructions: None,
        run_id: None,
        conversation_id: None,
        metadata: serde_json::Map::new(),
    })]
}

fn settings() -> ModelSettings {
    ModelSettings {
        max_tokens: Some(64),
        temperature: Some(0.2),
        ..ModelSettings::default()
    }
}

fn tools() -> Vec<ToolDefinition> {
    vec![ToolDefinition {
        name: "lookup".to_string(),
        description: Some("Look up a value".to_string()),
        parameters: json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
            "required": ["query"]
        }),
        metadata: serde_json::Map::new(),
    }]
}

fn assert_json_eq(actual: &Value, expected: &Value) {
    assert_eq!(actual, expected);
}

#[test]
fn replays_openai_chat_request_and_response() {
    let request = OpenAiChatAdapter::build_request(
        "gpt-4.1-mini",
        &canonical_history(),
        Some(&settings()),
        &tools(),
    )
    .unwrap();
    assert_json_eq(
        &request,
        &json!({
            "model": "gpt-4.1-mini",
            "messages": [
                {"role": "system", "content": "You are concise."},
                {"role": "user", "content": "What is 2+2?"}
            ],
            "max_tokens": 64,
            "temperature": 0.2,
            "tools": [{
                "type": "function",
                "function": {
                    "name": "lookup",
                    "description": "Look up a value",
                    "parameters": {
                        "type": "object",
                        "properties": {"query": {"type": "string"}},
                        "required": ["query"]
                    }
                }
            }]
        }),
    );

    let response = OpenAiChatAdapter::parse_response(&json!({
        "id": "chatcmpl_1",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "4"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
    }))
    .unwrap();
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 11);
}

#[test]
fn replays_openai_responses_request_and_tool_response() {
    let request = OpenAiResponsesAdapter::build_request(
        "gpt-4.1-mini",
        &canonical_history(),
        Some(&settings()),
        &[],
        &[],
    )
    .unwrap();
    assert_json_eq(
        &request,
        &json!({
            "model": "gpt-4.1-mini",
            "instructions": "You are concise.",
            "input": [{"role": "user", "content": [{"type": "input_text", "text": "What is 2+2?"}]}],
            "max_tokens": 64,
            "temperature": 0.2
        }),
    );

    let response = OpenAiResponsesAdapter::parse_response(&json!({
        "id": "resp_1",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [
            {"type": "message", "content": [{"type": "output_text", "text": "Need lookup"}]},
            {"type": "function_call", "call_id": "call_1", "name": "lookup", "arguments": "{\"query\":\"2+2\"}"}
        ],
        "usage": {"input_tokens": 7, "output_tokens": 4, "total_tokens": 11}
    }))
    .unwrap();
    assert_eq!(response.text_output(), "Need lookup");
    assert!(matches!(response.parts[1], ModelResponsePart::ToolCall(_)));
}

#[test]
fn maps_native_tools_to_openai_responses_tools() {
    let mut config = serde_json::Map::new();
    config.insert("search_context_size".to_string(), json!("low"));
    let request = OpenAiResponsesAdapter::build_request(
        "gpt-4.1-mini",
        &canonical_history(),
        None,
        &tools(),
        &[NativeToolDefinition::new("web_search_preview").with_config(config)],
    )
    .unwrap();

    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["name"], "lookup");
    assert_eq!(request["tools"][1]["type"], "web_search_preview");
    assert_eq!(request["tools"][1]["search_context_size"], "low");
}

#[test]
fn replays_anthropic_request_and_response() {
    let request = AnthropicMessagesAdapter::build_request(
        "claude-sonnet-4",
        &canonical_history(),
        Some(&settings()),
        &[],
    )
    .unwrap();
    assert_json_eq(
        &request,
        &json!({
            "model": "claude-sonnet-4",
            "system": "You are concise.",
            "messages": [{"role": "user", "content": [{"type": "text", "text": "What is 2+2?"}]}],
            "max_tokens": 64,
            "temperature": 0.2
        }),
    );

    let response = AnthropicMessagesAdapter::parse_response(&json!({
        "id": "msg_1",
        "model": "claude-sonnet-4",
        "content": [{"type": "text", "text": "4"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 9, "output_tokens": 1}
    }))
    .unwrap();
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 10);
}

#[test]
fn replays_gemini_request_and_response() {
    let request =
        GeminiGenerateContentAdapter::build_request(&canonical_history(), Some(&settings()), &[])
            .unwrap();
    assert_json_eq(
        &request,
        &json!({
            "systemInstruction": {"parts": [{"text": "You are concise."}]},
            "contents": [{"role": "user", "parts": [{"text": "What is 2+2?"}]}],
            "generationConfig": {"maxOutputTokens": 64, "temperature": 0.2}
        }),
    );

    let response = GeminiGenerateContentAdapter::parse_response(&json!({
        "candidates": [{
            "finishReason": "STOP",
            "content": {"role": "model", "parts": [{"text": "4"}]}
        }],
        "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 1, "totalTokens": 6}
    }))
    .unwrap();
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 6);
}

#[test]
fn replays_bedrock_request_and_response() {
    let request = BedrockConverseAdapter::build_request(
        "anthropic.claude-3-5-sonnet",
        &canonical_history(),
        Some(&settings()),
        &[],
    )
    .unwrap();
    assert_json_eq(
        &request,
        &json!({
            "modelId": "anthropic.claude-3-5-sonnet",
            "system": [{"text": "You are concise."}],
            "messages": [{"role": "user", "content": [{"text": "What is 2+2?"}]}],
            "inferenceConfig": {"maxTokens": 64, "temperature": 0.2}
        }),
    );

    let response = BedrockConverseAdapter::parse_response(&json!({
        "output": {"message": {"role": "assistant", "content": [{"text": "4"}]}},
        "stopReason": "end_turn",
        "usage": {"inputTokens": 8, "outputTokens": 1, "totalTokens": 9},
        "ResponseMetadata": {"RequestId": "aws_1"}
    }))
    .unwrap();
    assert_eq!(response.text_output(), "4");
    assert_eq!(response.usage.total_tokens, 9);
}

#[test]
fn canonical_messages_round_trip() {
    let history = canonical_history();
    let encoded = serde_json::to_string_pretty(&history).unwrap();
    let decoded: Vec<ModelMessage> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, history);
}
