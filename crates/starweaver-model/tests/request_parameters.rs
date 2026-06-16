#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    get_model_settings, ContentPart, HttpModelConfig, HttpRequest, HttpRequestOptions,
    HttpResponse, MessageNormalization, ModelAdapter, ModelError, ModelEventStream,
    ModelHttpClient, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelSettings, NativeToolDefinition, OutputMode,
    ProtocolFamily, ProtocolModelClient, StructuredOutputMode, ToolChoice, ToolDefinition,
};

#[derive(Clone, Default)]
struct CaptureHttpClient {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    response: Arc<Mutex<Option<HttpResponse>>>,
    stream_events: Arc<Mutex<Vec<Value>>>,
}

impl CaptureHttpClient {
    fn with_response(response: HttpResponse) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response: Arc::new(Mutex::new(Some(response))),
            stream_events: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_stream_events(events: Vec<Value>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response: Arc::new(Mutex::new(None)),
            stream_events: Arc::new(Mutex::new(events)),
        }
    }

    fn last_request(&self) -> HttpRequest {
        self.requests.lock().unwrap().last().cloned().unwrap()
    }
}

#[async_trait]
impl ModelHttpClient for CaptureHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        Ok(self.response.lock().unwrap().clone().unwrap())
    }

    async fn send_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        self.requests.lock().unwrap().push(request);
        let events = self.stream_events.lock().unwrap().clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(events.len().max(1));
        tokio::spawn(async move {
            for event in events {
                let _ = sender.send(Ok(event)).await;
            }
        });
        Ok(ModelEventStream::new(receiver))
    }
}

fn history() -> Vec<ModelMessage> {
    vec![ModelMessage::Request(ModelRequest {
        parts: vec![
            ModelRequestPart::SystemPrompt {
                text: "You are concise.".to_string(),
                metadata: Map::new(),
            },
            ModelRequestPart::UserPrompt {
                content: vec![ContentPart::Text {
                    text: "What is 2+2?".to_string(),
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
    })]
}

fn context() -> ModelRequestContext {
    ModelRequestContext::new(
        RunId::from_string("run_params"),
        ConversationId::from_string("conv_params"),
    )
}

fn context_with_metadata(metadata: Map<String, Value>) -> ModelRequestContext {
    context().with_llm_trace_metadata(metadata)
}

const fn text_response(body: Value) -> HttpResponse {
    HttpResponse::ok(body)
}

#[test]
fn model_request_parameters_serialize_round_trip() {
    let mut native_config = Map::new();
    native_config.insert("search_context_size".to_string(), json!("low"));
    let mut native_metadata = Map::new();
    native_metadata.insert("source".to_string(), json!("builtin"));

    let mut extra_body = Map::new();
    extra_body.insert("metadata".to_string(), json!({"tenant": "default"}));

    let params = ModelRequestParameters {
        tools: vec![ToolDefinition {
            name: "lookup".to_string(),
            description: Some("Look up a value".to_string()),
            parameters: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
            metadata: Map::from_iter([("tier".to_string(), json!("required"))]),
        }],
        native_tools: vec![NativeToolDefinition::new("web_search_preview")
            .with_config(native_config)
            .with_metadata(native_metadata)],
        output_schema: Some(json!({
            "name": "answer",
            "schema": {"type": "object", "required": ["answer"]},
            "strict": true
        })),
        http: HttpRequestOptions {
            headers: BTreeMap::from([("x-trace-id".to_string(), "trace_1".to_string())]),
            extra_body: Map::from_iter([("audit".to_string(), json!({"mode": "strict"}))]),
            endpoint_url: Some("https://gateway.example.test/v1/responses".to_string()),
            timeout_ms: Some(42_000),
            metadata: Map::from_iter([("route".to_string(), json!("audit"))]),
        },
        extra_body,
        ..ModelRequestParameters::default()
    };

    let encoded = serde_json::to_value(&params).unwrap();
    let decoded: ModelRequestParameters = serde_json::from_value(encoded.clone()).unwrap();

    assert_eq!(decoded, params);
    assert_eq!(encoded["tools"][0]["name"], "lookup");
    assert_eq!(
        encoded["native_tools"][0]["tool_type"],
        "web_search_preview"
    );
    assert_eq!(encoded["output_schema"]["name"], "answer");
    assert_eq!(
        encoded["http"]["endpoint_url"],
        "https://gateway.example.test/v1/responses"
    );
}

#[test]
fn model_settings_merge_preserves_defaults_and_overlays_request_values() {
    let base = ModelSettings {
        max_tokens: Some(128),
        temperature: Some(0.2),
        tool_choice: Some(ToolChoice::Auto),
        stop_sequences: vec!["base".to_string()],
        extra_headers: BTreeMap::from([("x-default".to_string(), "yes".to_string())]),
        extra_body: Map::from_iter([("default_body".to_string(), json!(true))]),
        ..ModelSettings::default()
    };
    let overlay = ModelSettings {
        temperature: Some(0.7),
        top_p: Some(0.9),
        tool_choice: Some(ToolChoice::Tool {
            name: "lookup".to_string(),
        }),
        extra_headers: BTreeMap::from([("x-request".to_string(), "yes".to_string())]),
        extra_body: Map::from_iter([("request_body".to_string(), json!(true))]),
        ..ModelSettings::default()
    };

    let merged = base.merge(&overlay);

    assert_eq!(merged.max_tokens, Some(128));
    assert_eq!(merged.temperature, Some(0.7));
    assert_eq!(merged.top_p, Some(0.9));
    assert_eq!(
        merged.tool_choice,
        Some(ToolChoice::Tool {
            name: "lookup".to_string()
        })
    );
    assert_eq!(merged.stop_sequences, vec!["base"]);
    assert_eq!(merged.extra_headers["x-default"], "yes");
    assert_eq!(merged.extra_headers["x-request"], "yes");
    assert_eq!(merged.extra_body["default_body"], true);
    assert_eq!(merged.extra_body["request_body"], true);
}

#[test]
fn profile_defaults_encode_provider_capability_contracts() {
    let openai_chat = ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
    assert_eq!(
        openai_chat.default_structured_output_mode,
        StructuredOutputMode::NativeJsonSchema
    );
    assert_eq!(
        openai_chat.message_normalization,
        MessageNormalization::MergeAdjacentSameRole
    );
    assert!(openai_chat.supports_tools);
    assert!(openai_chat.supports_json_schema_output);
    assert!(openai_chat.supports_image_input);

    let openai_responses = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
    assert_eq!(
        openai_responses.default_structured_output_mode,
        StructuredOutputMode::NativeJsonSchema
    );
    assert_eq!(
        openai_responses.message_normalization,
        MessageNormalization::PreserveItems
    );
    assert!(openai_responses.supports_thinking);
    assert!(openai_responses.supports_image_input);

    let anthropic = ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages);
    assert_eq!(
        anthropic.default_structured_output_mode,
        StructuredOutputMode::Tool
    );
    assert_eq!(
        anthropic.message_normalization,
        MessageNormalization::SystemField
    );
    assert!(anthropic.supports_thinking);
    assert!(anthropic.supports_image_input);
    assert!(anthropic.supports_document_input);

    let gemini = ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent);
    assert_eq!(
        gemini.message_normalization,
        MessageNormalization::SystemInstruction
    );
    assert!(gemini.supports_json_object_output);
    assert!(gemini.supports_image_input);
    assert!(gemini.supports_video_input);
    assert!(gemini.supports_audio_input);
    assert!(gemini.supports_document_input);

    let bedrock = ModelProfile::for_protocol(ProtocolFamily::BedrockConverse);
    assert_eq!(
        bedrock.default_structured_output_mode,
        StructuredOutputMode::Tool
    );
    assert_eq!(
        bedrock.message_normalization,
        MessageNormalization::SystemField
    );
    assert!(bedrock.supports_image_input);
    assert!(bedrock.supports_document_input);
}

#[tokio::test]
async fn protocol_client_merges_default_and_request_settings_into_wire_request() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_settings",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "4"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
    })));
    let default_settings = ModelSettings {
        max_tokens: Some(128),
        temperature: Some(0.2),
        extra_headers: BTreeMap::from([("x-default".to_string(), "yes".to_string())]),
        extra_body: Map::from_iter([("default_body".to_string(), json!(true))]),
        ..ModelSettings::default()
    };
    let request_settings = ModelSettings {
        temperature: Some(0.7),
        top_p: Some(0.9),
        extra_headers: BTreeMap::from([("x-request".to_string(), "yes".to_string())]),
        extra_body: Map::from_iter([("request_body".to_string(), json!(true))]),
        ..ModelSettings::default()
    };
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://api.openai.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    )
    .with_default_settings(default_settings);

    let response = client
        .request(
            history(),
            Some(request_settings),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "4");
    let request = http.last_request();
    assert_eq!(request.body["max_tokens"], 128);
    assert_eq!(request.body["temperature"], 0.7);
    assert_eq!(request.body["top_p"], 0.9);
    assert_eq!(request.body["default_body"], true);
    assert_eq!(request.body["request_body"], true);
    assert_eq!(request.headers["x-default"], "yes");
    assert_eq!(request.headers["x-request"], "yes");
}

#[tokio::test]
async fn protocol_client_adds_openai_prompt_cache_routing_from_session_metadata() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_cache",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                provider_options: Some(json!({
                    "store": false,
                    "openai_include_encrypted_reasoning": true
                })),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([
                (
                    "starweaver.session_id".to_string(),
                    json!("session_143a9ff4-285b-4fe7-ad79-b1b291bbac44"),
                ),
                (
                    "starweaver.prompt_cache_retention".to_string(),
                    json!("24h"),
                ),
            ])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(
        request.body["prompt_cache_key"],
        "sw_session_143a9ff4-285b-4fe7-ad79-b1b291bbac44"
    );
    assert_eq!(request.body["prompt_cache_retention"], "24h");
    assert_eq!(request.body["store"], false);
    assert!(request
        .body
        .get("openai_include_encrypted_reasoning")
        .is_none());
}

#[tokio::test]
async fn protocol_client_preserves_config_level_openai_prompt_cache_settings() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_cache_config_explicit",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
    })));
    let mut config = HttpModelConfig::new("https://api.openai.test/v1", "responses");
    config
        .extra_body
        .insert("prompt_cache_key".to_string(), json!("config-level-key"));
    config
        .extra_body
        .insert("prompt_cache_retention".to_string(), json!("24h"));
    config.extra_body.insert(
        "openai_include_encrypted_reasoning".to_string(),
        json!(true),
    );
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([(
                "starweaver.session_id".to_string(),
                json!("session_should_not_override_config"),
            )])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["prompt_cache_key"], "config-level-key");
    assert_eq!(request.body["prompt_cache_retention"], "24h");
    assert!(request
        .body
        .get("openai_include_encrypted_reasoning")
        .is_none());
}

#[tokio::test]
async fn protocol_client_preserves_explicit_openai_prompt_cache_settings() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_cache_explicit",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                extra_body: Map::from_iter([
                    ("prompt_cache_key".to_string(), json!("custom-key")),
                    ("prompt_cache_retention".to_string(), json!("24h")),
                ]),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([(
                "starweaver.session_id".to_string(),
                json!("session_should_not_override"),
            )])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["prompt_cache_key"], "custom-key");
    assert_eq!(request.body["prompt_cache_retention"], "24h");
}

#[tokio::test]
async fn protocol_client_adds_openai_chat_prompt_cache_routing_from_session_metadata() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_cache",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "ok"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://api.openai.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([(
                "cli.session_id".to_string(),
                json!("session_chat_cache"),
            )])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["prompt_cache_key"], "sw_session_chat_cache");
}

#[tokio::test]
async fn protocol_client_does_not_add_session_prompt_cache_key_for_openai_compatible_models() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_compatible",
        "model": "mimo-v2.5-pro",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "ok"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "mimo-v2.5-pro",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://gateway.example.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([(
                "cli.session_id".to_string(),
                json!("session_compatible"),
            )])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert!(request.body.get("prompt_cache_key").is_none());
}

#[tokio::test]
async fn protocol_client_maps_output_schema_for_openai_responses() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_schema",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "{\"answer\":\"4\"}"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "name": "answer",
                    "schema": {"type": "object", "required": ["answer"]},
                    "strict": true,
                })),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["text"]["format"]["type"], "json_schema");
    assert_eq!(request.body["text"]["format"]["name"], "answer");
    assert_eq!(
        request.body["text"]["format"]["schema"]["required"],
        json!(["answer"])
    );
    assert_eq!(request.body["text"]["format"]["strict"], true);
}

#[tokio::test]
async fn protocol_client_maps_output_schema_for_openai_chat() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_schema",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "{\"answer\":\"4\"}"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://api.openai.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "name": "answer",
                    "schema": {"type": "object", "required": ["answer"]},
                    "strict": true,
                })),
                output_mode: Some(OutputMode::NativeJsonSchema),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["response_format"]["type"], "json_schema");
    assert_eq!(
        request.body["response_format"]["json_schema"]["name"],
        "answer"
    );
    assert_eq!(
        request.body["response_format"]["json_schema"]["schema"]["required"],
        json!(["answer"])
    );
}

#[tokio::test]
async fn protocol_client_keeps_prompted_output_schema_out_of_native_response_format() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_schema_prompted",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "{\"answer\":\"4\"}"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "name": "answer",
                    "schema": {"type": "object", "required": ["answer"]},
                    "strict": true,
                })),
                output_mode: Some(OutputMode::Prompted),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert!(request.body.get("text").is_none());
    assert!(request
        .body
        .get("instructions")
        .and_then(Value::as_str)
        .is_some_and(|instructions| instructions.contains("Always respond with a JSON object")));
}

#[tokio::test]
async fn protocol_client_keeps_prompted_output_schema_out_of_openai_chat_response_format() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_schema_prompted",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "{\"answer\":\"4\"}"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 4, "total_tokens": 14}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://api.openai.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {"answer": {"type": "string"}},
                    "required": ["answer"]
                })),
                output_mode: Some(OutputMode::Prompted),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert!(request.body.get("response_format").is_none());
    assert!(request.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |message| message["content"].as_str().is_some_and(|content| {
                content.contains("Always respond with a JSON object")
                    && content.contains("Don't include any text or Markdown fencing")
            })
        ));
}

#[tokio::test]
async fn protocol_client_stream_final_sets_stream_true_for_openai_responses() {
    let http = CaptureHttpClient::with_stream_events(vec![json!({
        "type": "response.completed",
        "response": {
            "id": "resp_stream_final",
            "model": "gpt-4.1-mini",
            "status": "completed",
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "streamed answer"}]
            }],
            "usage": {"input_tokens": 10, "output_tokens": 3, "total_tokens": 13}
        }
    })]);
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );

    let response = client
        .request_stream_final(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "schema": {"type": "object", "required": ["answer"]}
                })),
                output_mode: Some(OutputMode::Prompted),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "streamed answer");
    let request = http.last_request();
    assert_eq!(request.body["stream"], true);
    assert!(request.body.get("text").is_none());
}

#[tokio::test]
async fn protocol_client_maps_output_schema_for_gemini() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "candidates": [{
            "content": {"parts": [{"text": "{\"answer\":\"4\"}"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 3, "totalTokenCount": 8}
    })));
    let client = ProtocolModelClient::new(
        "gemini",
        "gemini-1.5-flash",
        ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
        HttpModelConfig::new(
            "https://generativelanguage.googleapis.com/v1beta",
            "models/gemini-1.5-flash:generateContent",
        ),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "schema": {"type": "object", "required": ["answer"]}
                })),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(
        request.body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(
        request.body["generationConfig"]["responseSchema"]["required"],
        json!(["answer"])
    );
}

#[tokio::test]
async fn protocol_client_keeps_prompted_output_schema_out_of_gemini_generation_config() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "candidates": [{
            "content": {"parts": [{"text": "{\"answer\":\"4\"}"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 3, "totalTokenCount": 8}
    })));
    let client = ProtocolModelClient::new(
        "gemini",
        "gemini-1.5-flash",
        ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
        HttpModelConfig::new(
            "https://generativelanguage.googleapis.com/v1beta",
            "models/gemini-1.5-flash:generateContent",
        ),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            None,
            ModelRequestParameters {
                output_schema: Some(json!({
                    "schema": {"type": "object", "required": ["answer"]}
                })),
                output_mode: Some(OutputMode::Prompted),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert!(request
        .body
        .get("generationConfig")
        .and_then(Value::as_object)
        .is_none_or(|config| !config.contains_key("responseSchema")));
    let system_instruction = request.body["systemInstruction"]["parts"][0]["text"]
        .as_str()
        .unwrap();
    assert!(system_instruction.contains("You are concise."));
    assert!(system_instruction.contains("Always respond with a JSON object"));
    assert!(system_instruction.contains("Don't include any text or Markdown fencing"));
}

#[tokio::test]
async fn anthropic_adaptive_presets_emit_effort_and_interleaved_beta() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "msg_adaptive",
        "model": "claude-sonnet",
        "content": [{"type": "text", "text": "ok"}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1, "output_tokens": 1}
    })));
    let client = ProtocolModelClient::new(
        "anthropic",
        "claude-sonnet",
        ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages),
        HttpModelConfig::new("https://api.anthropic.test/v1", "messages"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(get_model_settings("anthropic_adaptive_xhigh").unwrap()),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["thinking"]["type"], "adaptive");
    assert_eq!(request.body["thinking"]["display"], "summarized");
    assert_eq!(request.body["output_config"]["effort"], "xhigh");

    let interleaved = get_model_settings("anthropic_high_interleaved_thinking").unwrap();
    assert!(interleaved.extra_headers["anthropic-beta"].contains("interleaved-thinking"));
    assert_eq!(interleaved.thinking.unwrap().effort, "high");
}

#[tokio::test]
async fn openai_tool_choice_allowlists_and_none_are_prepared_for_wire_requests() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_tools",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    })));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        HttpModelConfig::new("https://api.openai.test/v1", "responses"),
        Arc::new(http.clone()),
    );
    let tools = vec![
        ToolDefinition {
            name: "lookup".to_string(),
            description: None,
            parameters: json!({"type": "object"}),
            metadata: Map::new(),
        },
        ToolDefinition {
            name: "skip".to_string(),
            description: None,
            parameters: json!({"type": "object"}),
            metadata: Map::new(),
        },
    ];

    client
        .request(
            history(),
            Some(ModelSettings {
                tool_choice: Some(ToolChoice::Tools {
                    names: vec!["lookup".to_string()],
                }),
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                tools: tools.clone(),
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();
    let request = http.last_request();
    assert_eq!(request.body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(request.body["tools"][0]["name"], "lookup");
    assert_eq!(request.body["tool_choice"]["type"], "allowed_tools");
    assert_eq!(request.body["tool_choice"]["mode"], "required");
    assert_eq!(request.body["tool_choice"]["tools"][0]["type"], "function");
    assert_eq!(request.body["tool_choice"]["tools"][0]["name"], "lookup");

    client
        .request(
            history(),
            Some(ModelSettings {
                tool_choice: Some(ToolChoice::None),
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                tools,
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();
    let request = http.last_request();
    assert!(request.body.get("tools").is_none());
    assert_eq!(request.body["tool_choice"], "none");
}
