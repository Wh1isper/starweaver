#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    get_model_settings, AnthropicSettings, BedrockSettings, CodexSettings, ContentPart,
    GatewaySettings, GoogleSettings, HttpModelConfig, HttpRequest, HttpRequestOptions,
    HttpResponse, MessageNormalization, ModelAdapter, ModelError, ModelEventStream,
    ModelHttpClient, ModelMessage, ModelProfile, ModelRequest, ModelRequestContext,
    ModelRequestParameters, ModelRequestPart, ModelSettings, NativeToolDefinition,
    OpenAiChatSettings, OpenAiResponsesSettings, OutputMode, ProtocolFamily, ProtocolModelClient,
    ProviderSettings, ServiceTier, StructuredOutputMode, ThinkingSettings, ToolChoice,
    ToolDefinition,
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
    assert!(openai_chat.drop_sampling_parameters_when_reasoning);

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
    assert!(openai_responses.drop_sampling_parameters_when_reasoning);
    assert!(openai_responses.supports_image_input);

    let anthropic = ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages);
    assert_eq!(
        anthropic.default_structured_output_mode,
        StructuredOutputMode::NativeJsonSchema
    );
    assert!(anthropic.supports_json_schema_output);
    assert_eq!(
        anthropic.message_normalization,
        MessageNormalization::SystemField
    );
    assert!(anthropic.supports_thinking);
    assert!(anthropic.drop_sampling_parameters_when_reasoning);
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
    assert_eq!(request.body["max_completion_tokens"], 128);
    assert!(request.body.get("max_tokens").is_none());
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

#[test]
fn provider_settings_merge_is_field_level_and_preserves_raw_overrides() {
    let base = ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                user: Some("base-user".to_string()),
                store: Some(false),
                prompt_cache_key: Some("base-cache".to_string()),
                ..OpenAiChatSettings::default()
            }),
            gateway: Some(GatewaySettings {
                x_session_id: Some("base-gateway".to_string()),
                extra_headers: BTreeMap::from([("x-base".to_string(), "yes".to_string())]),
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };
    let overlay = ModelSettings {
        provider_settings: ProviderSettings {
            openai_chat: Some(OpenAiChatSettings {
                logprobs: Some(true),
                prompt_cache_key: Some("overlay-cache".to_string()),
                ..OpenAiChatSettings::default()
            }),
            gateway: Some(GatewaySettings {
                x_session_id: None,
                extra_headers: BTreeMap::from([("x-overlay".to_string(), "yes".to_string())]),
            }),
            ..ProviderSettings::default()
        },
        ..ModelSettings::default()
    };

    let merged = base.merge(&overlay);
    let openai = merged.provider_settings.openai_chat.unwrap();
    assert_eq!(openai.user.as_deref(), Some("base-user"));
    assert_eq!(openai.store, Some(false));
    assert_eq!(openai.logprobs, Some(true));
    assert_eq!(openai.prompt_cache_key.as_deref(), Some("overlay-cache"));
    let gateway = merged.provider_settings.gateway.unwrap();
    assert_eq!(gateway.x_session_id.as_deref(), Some("base-gateway"));
    assert_eq!(gateway.extra_headers["x-base"], "yes");
    assert_eq!(gateway.extra_headers["x-overlay"], "yes");
}

#[test]
fn protocol_profiles_only_advertise_native_tools_consumed_by_mappers() {
    let openai_chat = ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions);
    assert!(openai_chat.supported_native_tools.is_empty());

    let openai_responses = ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses);
    assert!(!openai_responses.supported_native_tools.is_empty());

    let anthropic = ModelProfile::for_protocol(ProtocolFamily::AnthropicMessages);
    assert!(anthropic.supported_native_tools.is_empty());
    assert!(anthropic.supports_json_schema_output);
    assert_eq!(
        anthropic.default_structured_output_mode,
        StructuredOutputMode::NativeJsonSchema
    );
}

#[tokio::test]
async fn openai_chat_uses_max_completion_tokens_seed_and_typed_settings() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_typed",
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
            Some(ModelSettings {
                max_tokens: Some(64),
                seed: Some(42),
                provider_settings: ProviderSettings {
                    openai_chat: Some(OpenAiChatSettings {
                        user: Some("user-1".to_string()),
                        store: Some(false),
                        logprobs: Some(true),
                        top_logprobs: Some(2),
                        prompt_cache_key: Some("typed-cache".to_string()),
                        prompt_cache_retention: Some("24h".to_string()),
                        ..OpenAiChatSettings::default()
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context_with_metadata(Map::from_iter([(
                "cli.session_id".to_string(),
                json!("legacy-session"),
            )])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["max_completion_tokens"], 64);
    assert!(request.body.get("max_tokens").is_none());
    assert_eq!(request.body["seed"], 42);
    assert_eq!(request.body["user"], "user-1");
    assert_eq!(request.body["store"], false);
    assert_eq!(request.body["logprobs"], true);
    assert_eq!(request.body["top_logprobs"], 2);
    assert_eq!(request.body["prompt_cache_key"], "typed-cache");
    assert_eq!(request.body["prompt_cache_retention"], "24h");
}

#[tokio::test]
async fn openai_chat_drops_sampling_parameters_when_reasoning_policy_requires_it() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_reasoning",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "ok"}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
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
            Some(ModelSettings {
                max_tokens: Some(64),
                temperature: Some(0.2),
                top_p: Some(0.9),
                presence_penalty: Some(0.1),
                frequency_penalty: Some(0.2),
                logit_bias: BTreeMap::from([("42".to_string(), 1)]),
                thinking: Some(ThinkingSettings {
                    effort: "high".to_string(),
                    budget_tokens: None,
                    mode: None,
                    include_thoughts: None,
                    summary: None,
                }),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["max_completion_tokens"], 64);
    assert_eq!(request.body["reasoning_effort"], "high");
    assert!(request.body.get("temperature").is_none());
    assert!(request.body.get("top_p").is_none());
    assert!(request.body.get("presence_penalty").is_none());
    assert!(request.body.get("frequency_penalty").is_none());
    assert!(request.body.get("logit_bias").is_none());
}

#[tokio::test]
async fn openai_compatible_chat_provider_defaults_to_legacy_max_tokens() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "chatcmpl_gateway",
        "model": "gateway-model",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "ok"}}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    })));
    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gateway-model",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://gateway.example.test/v1", "chat/completions"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                max_tokens: Some(64),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["max_tokens"], 64);
    assert!(request.body.get("max_completion_tokens").is_none());
}

#[tokio::test]
async fn openai_responses_maps_typed_settings_and_preserves_raw_override() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_typed",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
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
                seed: Some(42),
                provider_settings: ProviderSettings {
                    openai_responses: Some(OpenAiResponsesSettings {
                        store: Some(false),
                        user: Some("user-2".to_string()),
                        truncation: Some("auto".to_string()),
                        text_verbosity: Some("low".to_string()),
                        context_management: Some(json!({"strategy": "retain"})),
                        include: vec!["reasoning.encrypted_content".to_string()],
                        prompt_cache_key: Some("typed-resp-cache".to_string()),
                        prompt_cache_retention: Some("24h".to_string()),
                    }),
                    ..ProviderSettings::default()
                },
                extra_body: Map::from_iter([("prompt_cache_key".to_string(), json!("raw-cache"))]),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["store"], false);
    assert_eq!(request.body["user"], "user-2");
    assert_eq!(request.body["truncation"], "auto");
    assert_eq!(request.body["text"]["verbosity"], "low");
    assert_eq!(request.body["context_management"]["strategy"], "retain");
    assert_eq!(request.body["include"][0], "reasoning.encrypted_content");
    assert_eq!(request.body["prompt_cache_key"], "raw-cache");
    assert_eq!(request.body["prompt_cache_retention"], "24h");
    assert!(request.body.get("seed").is_none());
}

#[tokio::test]
async fn openai_responses_drops_sampling_parameters_when_reasoning_policy_requires_it() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_reasoning",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
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
                max_tokens: Some(64),
                temperature: Some(0.2),
                top_p: Some(0.9),
                presence_penalty: Some(0.1),
                frequency_penalty: Some(0.2),
                logit_bias: BTreeMap::from([("42".to_string(), 1)]),
                thinking: Some(ThinkingSettings {
                    effort: "high".to_string(),
                    budget_tokens: None,
                    mode: None,
                    include_thoughts: None,
                    summary: Some("auto".to_string()),
                }),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["max_output_tokens"], 64);
    assert_eq!(request.body["reasoning"]["effort"], "high");
    assert_eq!(request.body["reasoning"]["summary"], "auto");
    assert!(request.body.get("temperature").is_none());
    assert!(request.body.get("top_p").is_none());
    assert!(request.body.get("presence_penalty").is_none());
    assert!(request.body.get("frequency_penalty").is_none());
    assert!(request.body.get("logit_bias").is_none());
}

#[tokio::test]
async fn gateway_and_codex_typed_routing_settings_drive_headers_and_metadata() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_gateway",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    })));
    let mut config = HttpModelConfig::new("https://gateway.example.test/v1", "responses");
    config
        .headers
        .insert("X-Session-ID".to_string(), "config-sticky".to_string());
    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                provider_settings: ProviderSettings {
                    gateway: Some(GatewaySettings {
                        x_session_id: Some("sticky-1".to_string()),
                        extra_headers: BTreeMap::from([(
                            "x-gateway-route".to_string(),
                            "route-a".to_string(),
                        )]),
                    }),
                    codex: Some(CodexSettings {
                        session_id: Some("typed-codex-session".to_string()),
                        thread_id: Some("typed-codex-thread".to_string()),
                    }),
                    ..ProviderSettings::default()
                },
                extra_headers: BTreeMap::from([(
                    "X-Session-ID".to_string(),
                    "raw-sticky".to_string(),
                )]),
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                http: HttpRequestOptions {
                    headers: BTreeMap::from([
                        ("x-session-id".to_string(), "request-sticky".to_string()),
                        ("x-request-route".to_string(), "request-route".to_string()),
                    ]),
                    ..HttpRequestOptions::default()
                },
                ..ModelRequestParameters::default()
            },
            context_with_metadata(Map::from_iter([
                (
                    "provider.codex.session_id".to_string(),
                    json!("legacy-session"),
                ),
                (
                    "provider.codex.thread_id".to_string(),
                    json!("legacy-thread"),
                ),
            ])),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.headers["x-session-id"], "request-sticky");
    assert!(!request.headers.contains_key("X-Session-ID"));
    assert_eq!(request.headers["x-gateway-route"], "route-a");
    assert_eq!(request.headers["x-request-route"], "request-route");
    assert_eq!(
        request.metadata["provider.codex.session_id"],
        "typed-codex-session"
    );
    assert_eq!(
        request.metadata["provider.codex.thread_id"],
        "typed-codex-thread"
    );
}

#[tokio::test]
async fn gateway_header_layers_override_case_insensitively_without_request_header() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "resp_gateway_layers",
        "model": "gpt-5.5",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "ok"}]}],
        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
    })));
    let mut config = HttpModelConfig::new("https://gateway.example.test/v1", "responses");
    config
        .headers
        .insert("X-Session-ID".to_string(), "config-sticky".to_string());
    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                provider_settings: ProviderSettings {
                    gateway: Some(GatewaySettings {
                        x_session_id: Some("typed-sticky".to_string()),
                        extra_headers: BTreeMap::new(),
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();
    let request = http.last_request();
    assert_eq!(request.headers["x-session-id"], "typed-sticky");
    assert!(!request.headers.contains_key("X-Session-ID"));

    client
        .request(
            history(),
            Some(ModelSettings {
                provider_settings: ProviderSettings {
                    gateway: Some(GatewaySettings {
                        x_session_id: Some("typed-sticky".to_string()),
                        extra_headers: BTreeMap::from([(
                            "X-SESSION-ID".to_string(),
                            "gateway-extra-sticky".to_string(),
                        )]),
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();
    let request = http.last_request();
    assert_eq!(request.headers["X-SESSION-ID"], "gateway-extra-sticky");
    assert!(!request.headers.contains_key("x-session-id"));
    assert!(!request.headers.contains_key("X-Session-ID"));

    client
        .request(
            history(),
            Some(ModelSettings {
                provider_settings: ProviderSettings {
                    gateway: Some(GatewaySettings {
                        x_session_id: Some("typed-sticky".to_string()),
                        extra_headers: BTreeMap::from([(
                            "X-SESSION-ID".to_string(),
                            "gateway-extra-sticky".to_string(),
                        )]),
                    }),
                    ..ProviderSettings::default()
                },
                extra_headers: BTreeMap::from([(
                    "x-session-id".to_string(),
                    "settings-sticky".to_string(),
                )]),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();
    let request = http.last_request();
    assert_eq!(request.headers["x-session-id"], "settings-sticky");
    assert!(!request.headers.contains_key("X-SESSION-ID"));
    assert!(!request.headers.contains_key("X-Session-ID"));
}

#[tokio::test]
async fn anthropic_maps_tool_choice_parallel_tools_and_native_structured_output() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "msg_typed",
        "model": "claude-sonnet",
        "content": [{"type": "text", "text": "{\"answer\":\"4\"}"}],
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
            Some(ModelSettings {
                tool_choice: Some(ToolChoice::Tool {
                    name: "lookup".to_string(),
                }),
                parallel_tool_calls: Some(false),
                thinking: Some(ThinkingSettings {
                    effort: "high".to_string(),
                    budget_tokens: None,
                    mode: Some("adaptive".to_string()),
                    include_thoughts: None,
                    summary: None,
                }),
                provider_settings: ProviderSettings {
                    anthropic: Some(AnthropicSettings {
                        metadata: Some(json!({"tenant": "default"})),
                        betas: vec!["fine-grained-tool-streaming-2025-05-14".to_string()],
                        service_tier: Some("priority".to_string()),
                        context_management: Some(json!({"edits": []})),
                        ..AnthropicSettings::default()
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                tools: vec![ToolDefinition {
                    name: "lookup".to_string(),
                    description: None,
                    parameters: json!({"type": "object"}),
                    metadata: Map::new(),
                }],
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
    assert_eq!(request.body["tool_choice"]["type"], "tool");
    assert_eq!(request.body["tool_choice"]["name"], "lookup");
    assert_eq!(
        request.body["tool_choice"]["disable_parallel_tool_use"],
        true
    );
    assert_eq!(request.body["output_config"]["effort"], "high");
    assert_eq!(
        request.body["output_config"]["format"]["type"],
        "json_schema"
    );
    assert_eq!(
        request.body["output_config"]["format"]["schema"]["required"],
        json!(["answer"])
    );
    assert_eq!(request.body["metadata"]["tenant"], "default");
    assert_eq!(
        request.headers["anthropic-beta"],
        "fine-grained-tool-streaming-2025-05-14"
    );
    assert_eq!(request.body["service_tier"], "priority");
    assert_eq!(request.body["context_management"]["edits"], json!([]));
}

#[tokio::test]
async fn anthropic_drops_sampling_parameters_when_thinking_policy_requires_it() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "msg_reasoning",
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
            Some(ModelSettings {
                max_tokens: Some(64),
                temperature: Some(0.2),
                top_p: Some(0.9),
                top_k: Some(50),
                thinking: Some(ThinkingSettings {
                    effort: "high".to_string(),
                    budget_tokens: Some(1024),
                    mode: Some("enabled".to_string()),
                    include_thoughts: None,
                    summary: None,
                }),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["max_tokens"], 64);
    assert_eq!(request.body["thinking"]["type"], "enabled");
    assert_eq!(request.body["thinking"]["budget_tokens"], 1024);
    assert!(request.body.get("temperature").is_none());
    assert!(request.body.get("top_p").is_none());
    assert!(request.body.get("top_k").is_none());
}

#[tokio::test]
async fn anthropic_preserves_sampling_parameters_when_thinking_is_disabled() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "id": "msg_reasoning_disabled",
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
            Some(ModelSettings {
                temperature: Some(0.2),
                top_p: Some(0.9),
                top_k: Some(50),
                thinking: Some(ThinkingSettings {
                    effort: "off".to_string(),
                    budget_tokens: None,
                    mode: Some("disabled".to_string()),
                    include_thoughts: None,
                    summary: None,
                }),
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["thinking"]["type"], "disabled");
    assert_eq!(request.body["temperature"], 0.2);
    assert_eq!(request.body["top_p"], 0.9);
    assert_eq!(request.body["top_k"], 50);
}

#[tokio::test]
async fn gemini_maps_seed_and_typed_google_settings() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "candidates": [{
            "content": {"parts": [{"text": "ok"}]},
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 1, "candidatesTokenCount": 1, "totalTokenCount": 2}
    })));
    let client = ProtocolModelClient::new(
        "gemini",
        "gemini-2.5-flash",
        ModelProfile::for_protocol(ProtocolFamily::GeminiGenerateContent),
        HttpModelConfig::new(
            "https://generativelanguage.googleapis.com/v1beta",
            "models/gemini-2.5-flash:generateContent",
        ),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                seed: Some(12345),
                provider_settings: ProviderSettings {
                    google: Some(GoogleSettings {
                        safety_settings: Some(json!([{"category": "HARM_CATEGORY_HARASSMENT", "threshold": "BLOCK_ONLY_HIGH"}])),
                        cached_content: Some("cachedContents/cache-1".to_string()),
                        labels: Some(json!({"app": "starweaver"})),
                        response_logprobs: Some(true),
                        logprobs: Some(3),
                        service_tier: Some("priority".to_string()),
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert_eq!(request.body["generationConfig"]["seed"], 12345);
    assert_eq!(request.body["generationConfig"]["responseLogprobs"], true);
    assert_eq!(request.body["generationConfig"]["logprobs"], 3);
    assert_eq!(
        request.body["safetySettings"][0]["threshold"],
        "BLOCK_ONLY_HIGH"
    );
    assert_eq!(request.body["cachedContent"], "cachedContents/cache-1");
    assert_eq!(request.body["labels"]["app"], "starweaver");
    assert_eq!(request.body["serviceTier"], "priority");
}

#[tokio::test]
async fn bedrock_maps_typed_fields_and_omits_tools_when_tool_choice_none() {
    let http = CaptureHttpClient::with_response(text_response(json!({
        "output": {"message": {"content": [{"text": "ok"}]}},
        "stopReason": "end_turn",
        "usage": {"inputTokens": 1, "outputTokens": 1, "totalTokens": 2}
    })));
    let client = ProtocolModelClient::new(
        "bedrock",
        "anthropic.claude-sonnet-4-v1:0",
        ModelProfile::for_protocol(ProtocolFamily::BedrockConverse),
        HttpModelConfig::new("https://bedrock-runtime.test", "model/converse"),
        Arc::new(http.clone()),
    );

    client
        .request(
            history(),
            Some(ModelSettings {
                top_k: Some(200),
                tool_choice: Some(ToolChoice::None),
                service_tier: Some(ServiceTier::Priority),
                thinking: Some(ThinkingSettings {
                    effort: "high".to_string(),
                    budget_tokens: Some(4000),
                    mode: Some("enabled".to_string()),
                    include_thoughts: None,
                    summary: None,
                }),
                provider_settings: ProviderSettings {
                    bedrock: Some(BedrockSettings {
                        guardrail_config: Some(json!({"guardrailIdentifier": "gr-1"})),
                        performance_config: Some(json!({"latency": "optimized"})),
                        request_metadata: Some(json!({"tenant": "default"})),
                        additional_model_response_field_paths: vec!["/stop_sequence".to_string()],
                        prompt_variables: Some(json!({"topic": {"text": "math"}})),
                        additional_model_request_fields: Some(
                            json!({"anthropic_version": "bedrock-2023-05-31"}),
                        ),
                        inference_profile: Some("profile-1".to_string()),
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters {
                tools: vec![ToolDefinition {
                    name: "lookup".to_string(),
                    description: None,
                    parameters: json!({"type": "object"}),
                    metadata: Map::new(),
                }],
                ..ModelRequestParameters::default()
            },
            context(),
        )
        .await
        .unwrap();

    let request = http.last_request();
    assert!(request.body.get("toolConfig").is_none());
    assert_eq!(request.body["additionalModelRequestFields"]["top_k"], 200);
    assert_eq!(
        request.body["additionalModelRequestFields"]["thinking"]["budget_tokens"],
        4000
    );
    assert_eq!(
        request.body["additionalModelRequestFields"]["anthropic_version"],
        "bedrock-2023-05-31"
    );
    assert_eq!(request.body["serviceTier"]["type"], "priority");
    assert_eq!(
        request.body["guardrailConfig"]["guardrailIdentifier"],
        "gr-1"
    );
    assert_eq!(request.body["performanceConfig"]["latency"], "optimized");
    assert_eq!(request.body["requestMetadata"]["tenant"], "default");
    assert_eq!(
        request.body["additionalModelResponseFieldPaths"],
        json!(["/stop_sequence"])
    );
    assert_eq!(request.body["promptVariables"]["topic"]["text"], "math");
    assert_eq!(request.body["modelId"], "profile-1");
    assert!(request.body.get("inferenceProfile").is_none());
}
