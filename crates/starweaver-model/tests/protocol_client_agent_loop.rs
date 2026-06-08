#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_core::{ConversationId, RunId, Usage};
use starweaver_model::{
    ContentPart, FinishReason, HttpModelConfig, HttpRequest, HttpResponse, ModelAdapter,
    ModelError, ModelHttpClient, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelResponse, ModelResponsePart, ProtocolFamily,
    ProtocolModelClient, ToolArguments, ToolCallPart, ToolDefinition, ToolReturnPart,
};

#[derive(Clone)]
struct CaptureHttpClient {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    response: HttpResponse,
}

impl CaptureHttpClient {
    fn new(response: HttpResponse) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response,
        }
    }

    fn last_body(&self) -> Value {
        self.requests
            .lock()
            .unwrap()
            .last()
            .map(|request| request.body.clone())
            .unwrap()
    }
}

#[async_trait]
impl ModelHttpClient for CaptureHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        Ok(self.response.clone())
    }
}

#[tokio::test]
async fn protocol_clients_preserve_full_agent_tool_loop_history() {
    for scenario in protocol_scenarios() {
        let http = CaptureHttpClient::new(scenario.response);
        let client = protocol_client(scenario.protocol, http.clone());

        let response = client
            .request(
                agent_loop_history(),
                None,
                ModelRequestParameters {
                    tools: vec![lookup_tool()],
                    ..ModelRequestParameters::default()
                },
                context(),
            )
            .await
            .unwrap();

        assert_eq!(response.text_output(), "done");
        let body = http.last_body();
        (scenario.assert_wire)(&body);
    }
}

#[tokio::test]
async fn anthropic_client_preserves_multimodal_image_and_document_parts() {
    let http = CaptureHttpClient::new(HttpResponse::ok(json!({
        "id": "msg_1",
        "model": "claude-test",
        "content": [{"type": "text", "text": "done"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    })));
    let client = protocol_client(ProtocolFamily::AnthropicMessages, http.clone());

    client
        .request(
            vec![ModelMessage::Request(ModelRequest {
                parts: vec![ModelRequestPart::UserPrompt {
                    content: vec![
                        ContentPart::Text {
                            text: "inspect".to_string(),
                        },
                        ContentPart::ImageUrl {
                            url: "https://example.test/cat.png".to_string(),
                        },
                        ContentPart::FileUrl {
                            url: "https://example.test/spec.pdf".to_string(),
                            media_type: "application/pdf".to_string(),
                        },
                    ],
                    name: None,
                    metadata: Map::new(),
                }],
                timestamp: None,
                instructions: None,
                run_id: None,
                conversation_id: None,
                metadata: Map::new(),
            })],
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let content = http.last_body()["messages"][0]["content"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(content[0], json!({"type": "text", "text": "inspect"}));
    assert_eq!(
        content[1],
        json!({
            "type": "image",
            "source": {"type": "url", "url": "https://example.test/cat.png"}
        })
    );
    assert_eq!(
        content[2],
        json!({
            "type": "document",
            "source": {"type": "url", "url": "https://example.test/spec.pdf"}
        })
    );
}

struct ProtocolScenario {
    protocol: ProtocolFamily,
    response: HttpResponse,
    assert_wire: fn(&Value),
}

fn protocol_scenarios() -> Vec<ProtocolScenario> {
    vec![
        ProtocolScenario {
            protocol: ProtocolFamily::OpenAiChatCompletions,
            response: HttpResponse::ok(json!({
                "id": "chatcmpl_1",
                "model": "gpt-test",
                "choices": [{
                    "finish_reason": "stop",
                    "message": {"role": "assistant", "content": "done"}
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })),
            assert_wire: assert_openai_chat_wire,
        },
        ProtocolScenario {
            protocol: ProtocolFamily::OpenAiResponses,
            response: HttpResponse::ok(json!({
                "id": "resp_1",
                "model": "gpt-test",
                "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "done"}]}],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            })),
            assert_wire: assert_openai_responses_wire,
        },
        ProtocolScenario {
            protocol: ProtocolFamily::AnthropicMessages,
            response: HttpResponse::ok(json!({
                "id": "msg_1",
                "model": "claude-test",
                "content": [{"type": "text", "text": "done"}],
                "stop_reason": "end_turn",
                "usage": {"input_tokens": 1, "output_tokens": 1}
            })),
            assert_wire: assert_anthropic_wire,
        },
        ProtocolScenario {
            protocol: ProtocolFamily::GeminiGenerateContent,
            response: HttpResponse::ok(json!({
                "candidates": [{"finishReason": "STOP", "content": {"role": "model", "parts": [{"text": "done"}]}}],
                "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokens": 2}
            })),
            assert_wire: assert_gemini_wire,
        },
        ProtocolScenario {
            protocol: ProtocolFamily::BedrockConverse,
            response: HttpResponse::ok(json!({
                "output": {"message": {"role": "assistant", "content": [{"text": "done"}]}},
                "stopReason": "end_turn",
                "usage": {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2}
            })),
            assert_wire: assert_bedrock_wire,
        },
    ]
}

fn assert_openai_chat_wire(body: &Value) {
    let messages = body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "system"
            && message["content"]
                .as_str()
                .unwrap()
                .contains("request-level instruction")
    }));
    assert!(messages
        .iter()
        .any(|message| { message["role"] == "user" && message["content"] == "lookup Paris" }));
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["tool_calls"][0]["id"] == "call_1"
            && message["tool_calls"][0]["function"]["name"] == "lookup"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "call_1"
            && message["content"].as_str().unwrap().contains("capital")
    }));
    assert_eq!(body["tools"][0]["function"]["name"], "lookup");
}

fn assert_openai_responses_wire(body: &Value) {
    assert!(body["instructions"]
        .as_str()
        .unwrap()
        .contains("request-level instruction"));
    let input = body["input"].as_array().unwrap();
    assert!(input
        .iter()
        .any(|item| { item["role"] == "user" && item["content"][0]["text"] == "lookup Paris" }));
    assert!(input.iter().any(|item| {
        item["type"] == "function_call" && item["call_id"] == "call_1" && item["name"] == "lookup"
    }));
    assert!(input.iter().any(|item| {
        item["type"] == "function_call_output"
            && item["call_id"] == "call_1"
            && item["output"].as_str().unwrap().contains("capital")
    }));
    assert_eq!(body["tools"][0]["name"], "lookup");
}

fn assert_anthropic_wire(body: &Value) {
    assert!(body["system"]
        .as_str()
        .unwrap()
        .contains("request-level instruction"));
    let messages = body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"][0]["text"] == "lookup Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"][0]["type"] == "tool_use"
            && message["content"][0]["id"] == "call_1"
            && message["content"][0]["name"] == "lookup"
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
    assert_eq!(body["tools"][0]["name"], "lookup");
}

fn assert_gemini_wire(body: &Value) {
    assert!(body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap()
        .contains("request-level instruction"));
    let contents = body["contents"].as_array().unwrap();
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
        body["tools"][0]["functionDeclarations"][0]["name"],
        "lookup"
    );
}

fn assert_bedrock_wire(body: &Value) {
    assert!(body["system"][0]["text"]
        .as_str()
        .unwrap()
        .contains("request-level instruction"));
    let messages = body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"][0]["text"] == "lookup Paris"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant"
            && message["content"][0]["toolUse"]["toolUseId"] == "call_1"
            && message["content"][0]["toolUse"]["name"] == "lookup"
    }));
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"][0]["toolResult"]["toolUseId"] == "call_1"
            && message["content"][0]["toolResult"]["content"][0]["json"]["value"]
                == "Paris is the capital of France"
    }));
    assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["name"], "lookup");
}

fn protocol_client(protocol: ProtocolFamily, http: CaptureHttpClient) -> ProtocolModelClient {
    ProtocolModelClient::new(
        "test",
        "test-model",
        ModelProfile::for_protocol(protocol),
        HttpModelConfig::new("https://example.test", "/v1/test"),
        Arc::new(http),
    )
}

fn lookup_tool() -> ToolDefinition {
    ToolDefinition {
        name: "lookup".to_string(),
        description: Some("Look up a city fact".to_string()),
        parameters: json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
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
                    text: "system instruction".to_string(),
                    metadata: Map::new(),
                },
                ModelRequestPart::UserPrompt {
                    content: vec![ContentPart::Text {
                        text: "lookup Paris".to_string(),
                    }],
                    name: None,
                    metadata: Map::new(),
                },
            ],
            timestamp: None,
            instructions: Some("request-level instruction".to_string()),
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

fn context() -> ModelRequestContext {
    ModelRequestContext::new(
        RunId::from_string("run_protocol_loop"),
        ConversationId::from_string("conv_protocol_loop"),
    )
}
