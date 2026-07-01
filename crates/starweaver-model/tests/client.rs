#![allow(missing_docs, clippy::unwrap_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Map, json};
use starweaver_core::{ConversationId, RunId};
use starweaver_model::{
    ModelAdapter, ModelError, ModelSettings, ProtocolModelClient, ProviderAlias,
    ProviderAliasRegistry,
    adapter::{ModelRequestContext, ModelRequestParameters},
    message::{ContentPart, ModelMessage, ModelRequest, ModelRequestPart},
    profile::{ModelProfile, ProtocolFamily},
    transport::{
        AuthConfig, HttpModelConfig, HttpRequest, HttpResponse, MaxTokensParameter,
        ModelEventStream, ModelHttpClient, NoopSleeper, RetryPolicy,
    },
};

#[derive(Clone, Default)]
struct CaptureHttpClient {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    response: Arc<Mutex<Option<HttpResponse>>>,
    stream_events: Arc<Mutex<Option<Vec<serde_json::Value>>>>,
}

impl CaptureHttpClient {
    fn with_response(response: HttpResponse) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response: Arc::new(Mutex::new(Some(response))),
            stream_events: Arc::new(Mutex::new(None)),
        }
    }

    fn with_stream_events(events: Vec<serde_json::Value>) -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
            response: Arc::new(Mutex::new(None)),
            stream_events: Arc::new(Mutex::new(Some(events))),
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
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        let events = self.stream_events.lock().unwrap().clone().unwrap();
        tokio::spawn(async move {
            for event in events {
                if sender.send(Ok(event)).await.is_err() {
                    return;
                }
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
        RunId::from_string("run_client"),
        ConversationId::from_string("conv_client"),
    )
}

#[tokio::test]
async fn protocol_client_sends_custom_headers_and_extra_body() {
    let http = CaptureHttpClient::with_response(HttpResponse::ok(json!({
        "id": "chatcmpl_1",
        "model": "gpt-4.1-mini",
        "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "4"}}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
    })));

    let mut config =
        HttpModelConfig::new("https://gateway.example.test/openai", "chat/completions");
    config.auth = Some(AuthConfig::Bearer {
        token: "provider-token".to_string(),
    });
    config
        .headers
        .insert("x-org-id".to_string(), "org_default".to_string());
    config
        .extra_body
        .insert("audit".to_string(), json!({"tenant": "default"}));

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        config,
        Arc::new(http.clone()),
    );

    let mut params = ModelRequestParameters::default();
    params
        .http
        .headers
        .insert("x-trace-id".to_string(), "trace_123".to_string());
    params
        .extra_body
        .insert("metadata".to_string(), json!({"request_id": "req_123"}));

    let mut settings = ModelSettings {
        max_tokens: Some(64),
        temperature: Some(0.2),
        ..ModelSettings::default()
    };
    settings
        .extra_headers
        .insert("x-audit-mode".to_string(), "strict".to_string());
    settings
        .extra_body
        .insert("gateway".to_string(), json!({"policy": "audit"}));

    let response = client
        .request(history(), Some(settings), params, context())
        .await
        .unwrap();

    assert_eq!(response.text_output(), "4");
    let request = http.last_request();
    assert_eq!(
        request.url,
        "https://gateway.example.test/openai/chat/completions"
    );
    assert_eq!(
        request.headers.get("authorization").unwrap(),
        "Bearer provider-token"
    );
    assert_eq!(request.headers.get("x-org-id").unwrap(), "org_default");
    assert_eq!(request.headers.get("x-audit-mode").unwrap(), "strict");
    assert_eq!(request.headers.get("x-trace-id").unwrap(), "trace_123");
    assert_eq!(request.body["model"], json!("gpt-4.1-mini"));
    assert_eq!(request.body["audit"], json!({"tenant": "default"}));
    assert_eq!(request.body["gateway"], json!({"policy": "audit"}));
    assert_eq!(request.body["metadata"], json!({"request_id": "req_123"}));
}

#[tokio::test]
async fn protocol_client_allows_endpoint_override_for_gateways() {
    let http = CaptureHttpClient::with_response(HttpResponse::ok(json!({
        "id": "resp_1",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "4"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
    })));

    let mut config = HttpModelConfig::new("https://api.openai.com/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: "provider-token".to_string(),
    });

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http.clone()),
    );

    let mut params = ModelRequestParameters::default();
    params.http.endpoint_url = Some("https://audit-gateway.example.test/v1/responses".to_string());
    params.http.timeout_ms = Some(42_000);
    params.http.headers = BTreeMap::from([("x-route".to_string(), "audit".to_string())]);

    let response = client
        .request(history(), None, params, context())
        .await
        .unwrap();

    assert_eq!(response.text_output(), "4");
    let request = http.last_request();
    assert_eq!(
        request.url,
        "https://audit-gateway.example.test/v1/responses"
    );
    assert_eq!(request.timeout.unwrap().as_millis(), 42_000);
    assert_eq!(request.headers.get("x-route").unwrap(), "audit");
}

#[tokio::test]
async fn protocol_client_streams_openai_responses_events() {
    let http = CaptureHttpClient::with_stream_events(vec![
        json!({"type": "response.output_text.delta", "delta": "hel"}),
        json!({"type": "response.output_text.delta", "delta": "lo"}),
        json!({"type": "response.output_text.done"}),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_stream",
                "model": "gpt-5.5",
                "status": "completed",
                "output": [{"type": "message", "content": [{"type": "output_text", "text": "hello"}]}],
                "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
            }
        }),
    ]);

    let mut config = HttpModelConfig::new("https://gateway.example.test/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: "provider-token".to_string(),
    });
    config.max_tokens_parameter = MaxTokensParameter::Omit;

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http.clone()),
    );

    let settings = ModelSettings {
        max_tokens: Some(1024),
        temperature: Some(0.2),
        ..ModelSettings::default()
    };
    let events = client
        .request_stream(
            history(),
            Some(settings),
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert!(matches!(
        events[0],
        starweaver_model::ModelResponseStreamEvent::PartStart(_)
    ));
    assert!(matches!(
        events[2],
        starweaver_model::ModelResponseStreamEvent::PartDelta(_)
    ));
    let final_text = events
        .iter()
        .find_map(|event| match event {
            starweaver_model::ModelResponseStreamEvent::FinalResult(response) => {
                Some(response.text_output())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(final_text, "hello");

    let request = http.last_request();
    assert_eq!(request.body["model"], json!("gpt-5.5"));
    assert_eq!(request.body["stream"], json!(true));
    assert_eq!(request.body["temperature"], json!(0.2));
    assert!(request.body.get("max_tokens").is_none());
    assert!(request.body.get("max_output_tokens").is_none());
}

#[tokio::test]
async fn protocol_client_uses_stream_text_when_openai_responses_completed_payload_is_empty() {
    let http = CaptureHttpClient::with_stream_events(vec![
        json!({"type": "response.output_text.delta", "delta": "Hello"}),
        json!({"type": "response.output_text.delta", "delta": "!"}),
        json!({"type": "response.output_text.done"}),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_stream_empty_output",
                "model": "gpt-5.5",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}
            }
        }),
    ]);

    let mut config = HttpModelConfig::new("https://gateway.example.test/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: "provider-token".to_string(),
    });

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http),
    );

    let events = client
        .request_stream(
            history(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    let final_response = events
        .iter()
        .find_map(|event| match event {
            starweaver_model::ModelResponseStreamEvent::FinalResult(response) => Some(response),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_response.text_output(), "Hello!");
    assert_eq!(final_response.usage.total_tokens, 12);
    assert_eq!(
        final_response
            .provider
            .as_ref()
            .unwrap()
            .response_id
            .as_deref(),
        Some("resp_stream_empty_output")
    );
}

#[tokio::test]
async fn protocol_client_streams_openai_responses_reasoning_summary_events() {
    let http = CaptureHttpClient::with_stream_events(vec![
        json!({"type": "response.reasoning_summary_text.delta", "delta": "inspect"}),
        json!({"type": "response.reasoning_summary_text.delta", "delta": " context"}),
        json!({"type": "response.reasoning_summary_text.done"}),
        json!({"type": "response.output_text.delta", "delta": "done"}),
        json!({"type": "response.output_text.done"}),
        json!({
            "type": "response.completed",
            "response": {
                "id": "resp_reasoning_stream",
                "model": "gpt-5.5",
                "status": "completed",
                "output": [],
                "usage": {"input_tokens": 10, "output_tokens": 2, "total_tokens": 12}
            }
        }),
    ]);

    let mut config = HttpModelConfig::new("https://gateway.example.test/v1", "responses");
    config.auth = Some(AuthConfig::Bearer {
        token: "provider-token".to_string(),
    });

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-5.5",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses),
        config,
        Arc::new(http),
    );

    let events = client
        .request_stream(
            history(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        starweaver_model::ModelResponseStreamEvent::PartStart(start)
            if start.part_kind == "thinking"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        starweaver_model::ModelResponseStreamEvent::PartDelta(delta)
            if matches!(&delta.delta, starweaver_model::StreamDelta::Thinking { text } if text == "inspect")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        starweaver_model::ModelResponseStreamEvent::PartEnd(end)
            if end.part_kind.as_deref() == Some("thinking")
    )));
    let final_response = events
        .iter()
        .find_map(|event| match event {
            starweaver_model::ModelResponseStreamEvent::FinalResult(response) => Some(response),
            _ => None,
        })
        .unwrap();
    assert_eq!(final_response.text_output(), "done");
    assert!(final_response.parts.iter().any(|part| matches!(
        part,
        starweaver_model::ModelResponsePart::ProviderThinking { text, provider, .. }
            if text == "inspect context" && provider.provider_name.as_deref() == Some("openai")
    )));
}

#[tokio::test]
async fn protocol_client_retries_transient_status_failures() {
    let http = SequenceHttpClient::new(vec![
        Err(ModelError::ProviderStatus {
            status: 429,
            body: json!({"error": "rate limited"}),
            retryable: true,
        }),
        Ok(HttpResponse::ok(json!({
            "id": "chatcmpl_retry",
            "model": "gpt-4.1-mini",
            "choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": "4"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 1, "total_tokens": 11}
        }))),
    ]);

    let mut config =
        HttpModelConfig::new("https://gateway.example.test/openai", "chat/completions");
    config.retry_policy = RetryPolicy {
        max_attempts: 2,
        base_delay_ms: 1,
        max_delay_ms: 1,
        retry_statuses: vec![429],
    };

    let client = ProtocolModelClient::new(
        "gateway-openai",
        "gpt-4.1-mini",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        config,
        Arc::new(http.clone()),
    )
    .with_sleeper(Arc::new(NoopSleeper));

    let response = client
        .request(
            history(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "4");
    assert_eq!(*http.attempts.lock().unwrap(), 2);
}

#[tokio::test]
async fn protocol_client_maps_output_schema_to_openai_chat_response_format() {
    let http = CaptureHttpClient::with_response(HttpResponse::ok(json!({
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

    let params = ModelRequestParameters {
        output_schema: Some(json!({
            "name": "answer",
            "schema": {"type": "object", "required": ["answer"]},
            "strict": true,
        })),
        ..ModelRequestParameters::default()
    };

    let response = client
        .request(history(), None, params, context())
        .await
        .unwrap();

    assert_eq!(response.text_output(), "{\"answer\":\"4\"}");
    let request = http.last_request();
    assert_eq!(request.body["response_format"]["type"], "json_schema");
    assert_eq!(
        request.body["response_format"]["json_schema"]["name"],
        "answer"
    );
}

#[tokio::test]
async fn provider_alias_registry_builds_protocol_client() {
    let http = CaptureHttpClient::with_response(HttpResponse::ok(json!({
        "id": "resp_alias",
        "model": "gpt-4.1-mini",
        "status": "completed",
        "output": [{"type": "message", "content": [{"type": "output_text", "text": "4"}]}],
        "usage": {"input_tokens": 10, "output_tokens": 1, "total_tokens": 11}
    })));

    let mut registry = ProviderAliasRegistry::new();
    let mut config = HttpModelConfig::new("https://gateway.example.test", "responses");
    config
        .headers
        .insert("x-provider-alias".to_string(), "audit-openai".to_string());
    registry.insert(ProviderAlias::new(
        "audit-openai",
        "gateway-openai",
        "gpt-4.1-mini",
        ProtocolFamily::OpenAiResponses,
        config,
    ));

    let client = registry
        .build_with_client("audit-openai", Arc::new(http.clone()))
        .unwrap();
    let response = client
        .request(
            history(),
            None,
            ModelRequestParameters::default(),
            context(),
        )
        .await
        .unwrap();

    assert_eq!(response.text_output(), "4");
    let request = http.last_request();
    assert_eq!(
        request.headers.get("x-provider-alias").unwrap(),
        "audit-openai"
    );
    assert_eq!(request.url, "https://gateway.example.test/responses");
}

#[derive(Clone)]
struct SequenceHttpClient {
    responses: Arc<Mutex<Vec<Result<HttpResponse, String>>>>,
    attempts: Arc<Mutex<u32>>,
}

impl SequenceHttpClient {
    fn new(responses: Vec<Result<HttpResponse, ModelError>>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(
                responses
                    .into_iter()
                    .map(|result| result.map_err(|err| err.to_string()))
                    .rev()
                    .collect(),
            )),
            attempts: Arc::new(Mutex::new(0)),
        }
    }
}

#[async_trait]
impl ModelHttpClient for SequenceHttpClient {
    async fn send(&self, _request: HttpRequest) -> Result<HttpResponse, ModelError> {
        *self.attempts.lock().unwrap() += 1;
        let result = self.responses.lock().unwrap().pop().unwrap();
        match result {
            Ok(response) => Ok(response),
            Err(error) => Err(ModelError::Transport(error)),
        }
    }
}

#[test]
fn protocol_client_with_profile_overrides_capabilities() {
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-test",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        HttpModelConfig::new("https://example.test", "responses"),
        Arc::new(CaptureHttpClient::default()),
    )
    .with_profile(ModelProfile::for_protocol(ProtocolFamily::OpenAiResponses));

    assert_eq!(client.profile().protocol, ProtocolFamily::OpenAiResponses);
}
