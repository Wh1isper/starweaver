#![allow(missing_docs, clippy::unwrap_used)]

use serde_json::{json, Map};
use starweaver_core::Usage;
use starweaver_model::{
    providers::{
        anthropic::AnthropicMessagesAdapter, bedrock::BedrockConverseAdapter,
        gemini::GeminiGenerateContentAdapter, openai_chat::OpenAiChatAdapter,
        openai_responses::OpenAiResponsesAdapter,
    },
    FinishReason, ModelMessage, ModelRequest, ModelRequestPart, ModelResponse, ModelResponsePart,
    ModelSettings, ToolArguments, ToolCallPart, ToolDefinition, ToolReturnPart,
};

fn lookup_tool() -> ToolDefinition {
    ToolDefinition {
        name: "lookup".to_string(),
        description: Some("Look up a city fact".to_string()),
        parameters: json!({
            "type": "object",
            "properties": {
                "query": {"type": "string"}
            },
            "required": ["query"]
        }),
        metadata: Map::new(),
    }
}

fn agent_loop_history() -> Vec<ModelMessage> {
    vec![
        ModelMessage::Request(ModelRequest {
            parts: vec![
                ModelRequestPart::SystemPrompt {
                    text: "Answer with tool evidence.".to_string(),
                    metadata: Map::new(),
                },
                ModelRequestPart::Instruction {
                    text: "Keep replies concise.".to_string(),
                    metadata: Map::new(),
                },
                ModelRequestPart::UserPrompt {
                    content: vec![starweaver_model::ContentPart::Text {
                        text: "lookup Paris".to_string(),
                    }],
                    name: None,
                    metadata: Map::new(),
                },
            ],
            timestamp: None,
            instructions: Some("You are a city assistant.".to_string()),
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
        ModelMessage::Response(ModelResponse {
            parts: vec![ModelResponsePart::ToolCall(ToolCallPart {
                id: "call_1".to_string(),
                name: "lookup".to_string(),
                arguments: ToolArguments::parsed(json!({"query": "Paris"})),
            })],
            usage: Usage::default(),
            model_name: None,
            provider: None,
            finish_reason: Some(FinishReason::ToolCalls),
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
        ModelMessage::Request(ModelRequest {
            parts: vec![ModelRequestPart::ToolReturn(ToolReturnPart::new(
                "call_1",
                "lookup",
                json!({"value": "Paris is the capital of France"}),
            ))],
            timestamp: None,
            instructions: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        }),
    ]
}

fn tool_calls(response: &ModelResponse) -> Vec<ToolCallPart> {
    response.tool_calls()
}

#[test]
fn openai_chat_preserves_agent_loop_boundaries() {
    let request =
        OpenAiChatAdapter::build_request("gpt-test", &agent_loop_history(), None, &[lookup_tool()])
            .unwrap();

    let messages = request["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "system" && message["content"] == "You are a city assistant."
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "system" && message["content"] == "Answer with tool evidence."
    }));
    assert!(messages
        .iter()
        .any(|message| { message["role"] == "user" && message["content"] == "lookup Paris" }));
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .unwrap();
    assert_eq!(assistant["tool_calls"][0]["id"], "call_1");
    assert_eq!(assistant["tool_calls"][0]["function"]["name"], "lookup");
    assert_eq!(
        assistant["tool_calls"][0]["function"]["arguments"],
        r#"{"query":"Paris"}"#
    );
    let tool_return = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .unwrap();
    assert_eq!(tool_return["tool_call_id"], "call_1");
    assert!(tool_return["content"].as_str().unwrap().contains("capital"));
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["function"]["name"], "lookup");
}

#[test]
fn openai_chat_parses_text_tool_call_usage_and_finish_reason() {
    let response = OpenAiChatAdapter::parse_response(&json!({
        "id": "chatcmpl_1",
        "model": "gpt-test",
        "choices": [{
            "finish_reason": "tool_calls",
            "message": {
                "role": "assistant",
                "content": "Need a lookup.",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "lookup",
                        "arguments": "{\"query\":\"Paris\"}"
                    }
                }]
            }
        }],
        "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8}
    }))
    .unwrap();

    assert_eq!(response.text_output(), "Need a lookup.");
    assert_eq!(response.provider.as_ref().unwrap().name, "openai");
    assert_eq!(
        response.provider.as_ref().unwrap().response_id.as_deref(),
        Some("chatcmpl_1")
    );
    assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(response.usage.total_tokens, 8);
    let calls = tool_calls(&response);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].name, "lookup");
    assert_eq!(calls[0].arguments.execution_value()["query"], "Paris");
}

#[test]
fn openai_responses_preserves_agent_loop_boundaries() {
    let request = OpenAiResponsesAdapter::build_request(
        "gpt-test",
        &agent_loop_history(),
        None,
        &[lookup_tool()],
        &[],
    )
    .unwrap();

    assert!(request["instructions"]
        .as_str()
        .unwrap()
        .contains("You are a city assistant."));
    assert!(request["instructions"]
        .as_str()
        .unwrap()
        .contains("Answer with tool evidence."));
    let input = request["input"].as_array().unwrap();
    assert!(input
        .iter()
        .any(|item| { item["role"] == "user" && item["content"][0]["text"] == "lookup Paris" }));
    assert!(input.iter().any(|item| {
        item["type"] == "function_call"
            && item["call_id"] == "call_1"
            && item["name"] == "lookup"
            && item["arguments"] == r#"{"query":"Paris"}"#
    }));
    assert!(input.iter().any(|item| {
        item["type"] == "function_call_output"
            && item["call_id"] == "call_1"
            && item["output"].as_str().unwrap().contains("capital")
    }));
    assert_eq!(request["tools"][0]["type"], "function");
    assert_eq!(request["tools"][0]["name"], "lookup");
}

#[test]
fn openai_responses_parses_text_tool_call_usage_and_finish_reason() {
    let response = OpenAiResponsesAdapter::parse_response(&json!({
        "id": "resp_1",
        "model": "gpt-test",
        "status": "completed",
        "output": [
            {
                "type": "message",
                "content": [{"type": "output_text", "text": "Need a lookup."}]
            },
            {
                "type": "function_call",
                "call_id": "call_1",
                "name": "lookup",
                "arguments": "{\"query\":\"Paris\"}"
            }
        ],
        "usage": {"input_tokens": 3, "output_tokens": 5, "total_tokens": 8}
    }))
    .unwrap();

    assert_eq!(response.text_output(), "Need a lookup.");
    assert_eq!(response.provider.as_ref().unwrap().name, "openai");
    assert_eq!(
        response.provider.as_ref().unwrap().response_id.as_deref(),
        Some("resp_1")
    );
    assert_eq!(response.finish_reason, Some(FinishReason::Stop));
    assert_eq!(response.usage.total_tokens, 8);
    let calls = tool_calls(&response);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].name, "lookup");
    assert_eq!(calls[0].arguments.execution_value()["query"], "Paris");
}

#[test]
fn anthropic_preserves_agent_loop_boundaries() {
    let request = AnthropicMessagesAdapter::build_request(
        "claude-test",
        &agent_loop_history(),
        None,
        &[lookup_tool()],
    )
    .unwrap();

    assert!(request["system"]
        .as_str()
        .unwrap()
        .contains("You are a city assistant."));
    assert!(request["system"]
        .as_str()
        .unwrap()
        .contains("Answer with tool evidence."));
    let messages = request["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"][0]["text"] == "lookup Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"][0]["type"] == "tool_use"
            && message["content"][0]["id"] == "call_1"
            && message["content"][0]["name"] == "lookup"
            && message["content"][0]["input"]["query"] == "Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"][0]["type"] == "tool_result"
            && message["content"][0]["tool_use_id"] == "call_1"
            && message["content"][0]["content"]
                .as_str()
                .unwrap()
                .contains("capital")
    }));
    assert_eq!(request["tools"][0]["name"], "lookup");
    assert_eq!(request["tools"][0]["input_schema"]["type"], "object");
}

#[test]
fn anthropic_caches_static_instruction_boundary_and_tool_definitions() {
    let mut dynamic_metadata = Map::new();
    dynamic_metadata.insert("starweaver_instruction_dynamic".to_string(), json!(true));
    let messages = vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "static system".to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::Instruction {
                text: "dynamic instruction".to_string(),
                metadata: dynamic_metadata,
            },
            ModelRequestPart::UserPrompt {
                content: vec![starweaver_model::ContentPart::Text {
                    text: "hello".to_string(),
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
    })];
    let settings = ModelSettings {
        provider_options: Some(json!({
            "anthropic_cache_instructions": true,
            "anthropic_cache_tool_definitions": true,
        })),
        ..ModelSettings::default()
    };

    let request = AnthropicMessagesAdapter::build_request(
        "claude-test",
        &messages,
        Some(&settings),
        &[lookup_tool()],
    )
    .unwrap();

    let system = request["system"].as_array().unwrap();
    assert_eq!(system[0]["text"], "static system");
    assert_eq!(system[0]["cache_control"]["type"], "ephemeral");
    assert_eq!(system[1]["text"], "dynamic instruction");
    assert!(system[1].get("cache_control").is_none());
    assert_eq!(request["tools"][0]["cache_control"]["type"], "ephemeral");
    assert!(request.get("anthropic_cache_instructions").is_none());
    assert!(request.get("anthropic_cache_tool_definitions").is_none());
}

#[test]
fn anthropic_parses_text_thinking_tool_call_usage_and_finish_reason() {
    let response = AnthropicMessagesAdapter::parse_response(&json!({
        "id": "msg_1",
        "model": "claude-test",
        "stop_reason": "tool_use",
        "content": [
            {"type": "thinking", "thinking": "inspect", "signature": "sig_1"},
            {"type": "text", "text": "Need a lookup."},
            {"type": "tool_use", "id": "call_1", "name": "lookup", "input": {"query": "Paris"}}
        ],
        "usage": {"input_tokens": 3, "output_tokens": 5}
    }))
    .unwrap();

    assert!(response.parts.iter().any(|part| matches!(
        part,
        ModelResponsePart::Thinking { text, signature }
            if text == "inspect" && signature.as_deref() == Some("sig_1")
    )));
    assert_eq!(response.text_output(), "Need a lookup.");
    assert_eq!(response.provider.as_ref().unwrap().name, "anthropic");
    assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(response.usage.total_tokens, 8);
    let calls = tool_calls(&response);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].arguments.execution_value()["query"], "Paris");
}

#[test]
fn gemini_preserves_agent_loop_boundaries() {
    let request =
        GeminiGenerateContentAdapter::build_request(&agent_loop_history(), None, &[lookup_tool()])
            .unwrap();

    assert!(request["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("You are a city assistant."));
    let contents = request["contents"].as_array().unwrap();
    assert!(contents.iter().any(|content| {
        content["role"] == "user" && content["parts"][0]["text"] == "lookup Paris"
    }));
    assert!(contents.iter().any(|content| {
        content["role"] == "model"
            && content["parts"][0]["functionCall"]["name"] == "lookup"
            && content["parts"][0]["functionCall"]["args"]["query"] == "Paris"
    }));
    assert!(contents.iter().any(|content| {
        content["role"] == "user"
            && content["parts"][0]["functionResponse"]["name"] == "lookup"
            && content["parts"][0]["functionResponse"]["response"]["content"]["value"]
                == "Paris is the capital of France"
    }));
    assert_eq!(
        request["tools"][0]["functionDeclarations"][0]["name"],
        "lookup"
    );
}

#[test]
fn gemini_parses_text_tool_call_usage_and_finish_reason() {
    let response = GeminiGenerateContentAdapter::parse_response(&json!({
        "candidates": [{
            "finishReason": "STOP",
            "content": {
                "role": "model",
                "parts": [
                    {"text": "Need a lookup."},
                    {"functionCall": {"id": "call_1", "name": "lookup", "args": {"query": "Paris"}}}
                ]
            }
        }],
        "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 5, "totalTokens": 8}
    }))
    .unwrap();

    assert_eq!(response.text_output(), "Need a lookup.");
    assert_eq!(response.provider.as_ref().unwrap().name, "gemini");
    assert_eq!(response.finish_reason, Some(FinishReason::Stop));
    assert_eq!(response.usage.total_tokens, 8);
    let calls = tool_calls(&response);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].arguments.execution_value()["query"], "Paris");
}

#[test]
fn bedrock_preserves_agent_loop_boundaries() {
    let request = BedrockConverseAdapter::build_request(
        "anthropic.claude-test",
        &agent_loop_history(),
        None,
        &[lookup_tool()],
    )
    .unwrap();

    assert!(request["system"][0]["text"]
        .as_str()
        .unwrap()
        .contains("You are a city assistant."));
    let messages = request["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"][0]["text"] == "lookup Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"][0]["toolUse"]["toolUseId"] == "call_1"
            && message["content"][0]["toolUse"]["name"] == "lookup"
            && message["content"][0]["toolUse"]["input"]["query"] == "Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"][0]["toolResult"]["toolUseId"] == "call_1"
            && message["content"][0]["toolResult"]["status"] == "success"
            && message["content"][0]["toolResult"]["content"][0]["json"]["value"]
                == "Paris is the capital of France"
    }));
    assert_eq!(
        request["toolConfig"]["tools"][0]["toolSpec"]["name"],
        "lookup"
    );
}

#[test]
fn bedrock_parses_text_tool_call_usage_and_finish_reason() {
    let response = BedrockConverseAdapter::parse_response(&json!({
        "output": {
            "message": {
                "role": "assistant",
                "content": [
                    {"text": "Need a lookup."},
                    {"toolUse": {"toolUseId": "call_1", "name": "lookup", "input": {"query": "Paris"}}}
                ]
            }
        },
        "stopReason": "tool_use",
        "usage": {"inputTokens": 3, "outputTokens": 5, "totalTokens": 8},
        "metrics": {"latencyMs": 10},
        "ResponseMetadata": {"RequestId": "aws_1"}
    }))
    .unwrap();

    assert_eq!(response.text_output(), "Need a lookup.");
    assert_eq!(response.provider.as_ref().unwrap().name, "bedrock");
    assert_eq!(
        response.provider.as_ref().unwrap().response_id.as_deref(),
        Some("aws_1")
    );
    assert_eq!(response.finish_reason, Some(FinishReason::ToolCalls));
    assert_eq!(response.usage.total_tokens, 8);
    let calls = tool_calls(&response);
    assert_eq!(calls[0].id, "call_1");
    assert_eq!(calls[0].arguments.execution_value()["query"], "Paris");
}
