#![allow(missing_docs, clippy::unwrap_used)]

use std::{collections::BTreeMap, sync::Arc, sync::Mutex};

use async_trait::async_trait;
use serde_json::{json, Map, Value};
use starweaver_core::{
    sdk_name, AgentId, CheckpointId, ConversationId, Metadata, RunId, SessionId,
    SubagentLifecycleEvent, SubagentLifecycleKind, SubagentSpec, TaskId, TraceContext,
};
use starweaver_model::transport::{
    build_http_request, is_retryable_status, merge_extra_body, send_with_retries,
    should_retry_error,
};
use starweaver_model::{
    allow_real_model_requests, allow_real_model_requests_guard, block_real_model_requests,
    detect_image_dimensions, gemini_http_config, get_model_config, get_model_settings,
    is_document_media_type, is_image_media_type, is_video_media_type, list_model_config_presets,
    list_model_settings_presets, model_runtime_preset, openai_chat_http_config,
    openai_responses_http_config, parse_data_url, set_allow_real_model_requests, AuthConfig,
    ContentPart, FinishReason, HttpModelConfig, HttpRequest, HttpRequestOptions, HttpResponse,
    MediaKind, MediaPolicy, MediaPreflight, ModelAdapter, ModelError, ModelHttpClient,
    ModelProfile, ModelRequestContext, ModelRequestParameters, ModelResponse,
    ModelResponseEventStream, ModelResponsePart, ModelResponseStreamEvent, ModelSettings,
    ModelSleeper, NoopSleeper, PartDelta, ProtocolFamily, ProtocolModelClient, RetryPolicy,
    ServiceTier,
};
use starweaver_usage::Usage;

#[derive(Default)]
struct SequenceHttpClient {
    requests: Mutex<Vec<HttpRequest>>,
    results: Mutex<Vec<Result<HttpResponse, ModelError>>>,
}

impl SequenceHttpClient {
    fn new(results: Vec<Result<HttpResponse, ModelError>>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            results: Mutex::new(results.into_iter().rev().collect()),
        }
    }

    fn requests(&self) -> Vec<HttpRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl ModelHttpClient for SequenceHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        self.requests.lock().unwrap().push(request);
        self.results.lock().unwrap().pop().unwrap()
    }
}

#[derive(Default)]
struct RecordingSleeper {
    durations: Mutex<Vec<std::time::Duration>>,
}

#[async_trait]
impl ModelSleeper for RecordingSleeper {
    async fn sleep(&self, duration: std::time::Duration) {
        self.durations.lock().unwrap().push(duration);
    }
}

#[test]
fn presets_cover_all_builtin_settings_configs_and_errors() {
    let required_settings = [
        "anthropic_adaptive_1m_cm_xhigh",
        "anthropic_1m_cm_low_interleaved_thinking",
        "anthropic_cm_off_interleaved_thinking",
        "openai_responses_default_fast",
        "openai_responses_xhigh_fast",
        "openai_responses_medium_fast",
        "openai_responses_low_fast",
        "deepseek_v4_default",
    ];
    let available_settings = list_model_settings_presets();
    for name in required_settings {
        assert!(
            available_settings.contains(&name.to_string()),
            "missing settings preset {name}"
        );
    }

    for name in list_model_settings_presets() {
        let settings = get_model_settings(&name).unwrap();
        match name.as_str() {
            "openai_responses_high_fast" => {
                assert_eq!(settings.service_tier, Some(ServiceTier::Priority));
            }
            "anthropic_cm" | "anthropic_cm_high" => {
                assert!(settings.extra_body.contains_key("context_management"));
            }
            "anthropic_1m" | "anthropic_1m_default" => {
                assert!(settings.extra_headers.contains_key("anthropic-beta"));
            }
            "deepseek_v4_off" => assert!(settings.thinking.is_none()),
            "mimo" | "mimo_v2_5" | "mimo_v2.5" | "mimo_v2_5_pro" | "mimo_v2.5_pro" => {
                assert_eq!(settings.extra_body["thinking"]["type"], "enabled");
            }
            "gemini_thinking_budget_default" | "gemini_2.5" => {
                assert_eq!(settings.thinking.unwrap().budget_tokens, Some(16 * 1024));
            }
            _ => {}
        }
    }

    for name in list_model_config_presets() {
        let config = get_model_config(&name).unwrap();
        assert!(config.context_window >= 200_000);
        match config.protocol {
            ProtocolFamily::AnthropicMessages => assert!(config.profile.supports_document_input),
            ProtocolFamily::GeminiGenerateContent => assert!(config.profile.supports_audio_input),
            ProtocolFamily::OpenAiChatCompletions
            | ProtocolFamily::OpenAiResponses
            | ProtocolFamily::BedrockConverse => {}
        }
    }

    assert!(get_model_settings("missing-preset").is_err());
    assert!(get_model_config("missing-config").is_err());

    let runtime = model_runtime_preset("gpt", "openai", "gpt-5", "openai", "gpt5").unwrap();
    let alias = runtime.provider_alias(openai_responses_http_config("token"));
    assert_eq!(alias.alias, "gpt");
    assert_eq!(alias.protocol, ProtocolFamily::OpenAiResponses);
}

#[test]
fn http_config_helpers_build_auth_endpoints_and_merge_requests() {
    let anthropic = starweaver_model::anthropic_http_config("ak");
    assert_eq!(
        anthropic.endpoint_url(),
        "https://api.anthropic.com/v1/messages"
    );
    assert_eq!(anthropic.headers["anthropic-version"], "2023-06-01");
    assert_eq!(
        anthropic.auth,
        Some(AuthConfig::Header {
            name: "x-api-key".to_string(),
            value: "ak".to_string()
        })
    );

    let openai_chat = openai_chat_http_config("sk");
    assert_eq!(
        openai_chat.endpoint_url(),
        "https://api.openai.com/v1/chat/completions"
    );
    assert!(matches!(openai_chat.auth, Some(AuthConfig::Bearer { .. })));

    let gemini = gemini_http_config("gk", "gemini-3-pro");
    assert_eq!(
        gemini.endpoint_url(),
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-3-pro:generateContent?key=gk"
    );

    let mut config = HttpModelConfig::new("https://gateway.example/v1/", "/chat");
    config.auth = Some(AuthConfig::Bearer {
        token: "base".to_string(),
    });
    config.headers.insert("x-base".to_string(), "1".to_string());
    config.extra_body.insert("base".to_string(), json!(true));
    config.timeout_ms = Some(1000);
    config
        .metadata
        .insert("source".to_string(), json!("config"));

    let options = HttpRequestOptions {
        headers: BTreeMap::from([("x-request".to_string(), "2".to_string())]),
        extra_body: Map::from_iter([("request".to_string(), json!(true))]),
        endpoint_url: Some("https://override.example/request".to_string()),
        timeout_ms: Some(2000),
        metadata: Map::from_iter([("route".to_string(), json!("override"))]),
    };
    let request = build_http_request(&config, &options, json!({"model": "m"}));
    assert_eq!(request.url, "https://override.example/request");
    assert_eq!(request.timeout.unwrap().as_millis(), 2000);
    assert_eq!(request.headers["authorization"], "Bearer base");
    assert_eq!(request.headers["x-base"], "1");
    assert_eq!(request.headers["x-request"], "2");
    assert_eq!(request.body["base"], true);
    assert_eq!(request.body["request"], true);
    assert_eq!(request.metadata["source"], "config");
    assert_eq!(request.metadata["route"], "override");

    assert_eq!(
        merge_extra_body(
            json!("scalar"),
            &Map::from_iter([("k".to_string(), json!(1))])
        ),
        json!("scalar")
    );
}

#[tokio::test]
async fn retry_policy_and_model_stream_helpers_cover_error_paths() {
    let policy = RetryPolicy {
        max_attempts: 3,
        base_delay_ms: 10,
        max_delay_ms: 15,
        retry_statuses: vec![418],
    };
    assert_eq!(policy.delay_for_attempt(1).as_millis(), 10);
    assert_eq!(policy.delay_for_attempt(5).as_millis(), 15);
    assert!(policy.retries_status(418));
    assert!(is_retryable_status(429));

    let retryable = ModelError::ProviderStatus {
        status: 418,
        body: json!({"error": "teapot"}),
        retryable: false,
    };
    assert!(should_retry_error(&retryable, &policy));
    assert!(!should_retry_error(
        &ModelError::ResponseParsing("bad".to_string()),
        &policy
    ));

    let client = SequenceHttpClient::new(vec![
        Err(ModelError::Transport("temporary".to_string())),
        Ok(HttpResponse::ok(json!({"ok": true}))),
    ]);
    let sleeper = RecordingSleeper::default();
    let request = build_http_request(
        &HttpModelConfig::new("https://api.example", "models"),
        &HttpRequestOptions::default(),
        json!({}),
    );
    let response = send_with_retries(&client, &sleeper, request.clone(), &policy)
        .await
        .unwrap();
    assert_eq!(response.body["ok"], true);
    assert_eq!(client.requests().len(), 2);
    assert_eq!(sleeper.durations.lock().unwrap().len(), 1);

    let exhausted = SequenceHttpClient::new(vec![
        Err(ModelError::Transport("a".to_string())),
        Err(ModelError::Transport("b".to_string())),
    ]);
    let error = send_with_retries(
        &exhausted,
        &NoopSleeper,
        request,
        &RetryPolicy {
            max_attempts: 2,
            ..policy
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(
        error,
        ModelError::RetryExhausted { attempts: 2, .. }
    ));

    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let mut stream = ModelResponseEventStream::new(receiver);
    sender
        .send(Ok(ModelResponseStreamEvent::PartDelta(PartDelta::text(
            0, "delta",
        ))))
        .await
        .unwrap();
    drop(sender);
    assert!(matches!(
        stream.recv().await.unwrap().unwrap(),
        ModelResponseStreamEvent::PartDelta(_)
    ));
    assert!(stream.recv().await.is_none());
}

#[test]
fn media_helpers_cover_policy_data_url_webp_and_corruption_edges() {
    assert!(is_image_media_type("image/png"));
    assert!(is_video_media_type("video/mp4"));
    assert!(is_document_media_type("application/pdf"));
    assert!(parse_data_url("data:image/png,iVBORw0KGgo=").is_err());
    assert!(parse_data_url("data:image/png;base64,***").is_err());

    let no_images = MediaPolicy {
        allow_images: false,
        ..MediaPolicy::default()
    };
    assert_eq!(
        MediaPreflight::inspect_with_policy(&png_bytes(1, 1), Some("image/png"), &no_images)
            .policy_reason
            .as_deref(),
        Some("image media is disabled by policy")
    );

    let no_webp = MediaPolicy {
        allow_webp: false,
        ..MediaPolicy::default()
    };
    assert_eq!(
        MediaPreflight::inspect_with_policy(
            &webp_vp8x_bytes(300, 200),
            Some("image/webp"),
            &no_webp
        )
        .policy_reason
        .as_deref(),
        Some("webp media is disabled by policy")
    );

    let no_videos = MediaPolicy {
        allow_videos: false,
        ..MediaPolicy::default()
    };
    assert_eq!(
        MediaPreflight::inspect_with_policy(
            b"\0\0\0\x18ftypmp42rest",
            Some("video/mp4"),
            &no_videos
        )
        .policy_reason
        .as_deref(),
        Some("video media is disabled by policy")
    );

    let no_docs = MediaPolicy {
        allow_documents: false,
        ..MediaPolicy::default()
    };
    assert_eq!(
        MediaPreflight::inspect_with_policy(b"%PDF", Some("application/pdf"), &no_docs)
            .policy_reason
            .as_deref(),
        Some("document media is disabled by policy")
    );

    assert_eq!(
        detect_image_dimensions(&webp_vp8x_bytes(300, 200), MediaKind::Webp)
            .unwrap()
            .width,
        300
    );
    assert_eq!(
        detect_image_dimensions(&webp_vp8l_bytes(5, 7), MediaKind::Webp)
            .unwrap()
            .height,
        7
    );
    assert_eq!(
        detect_image_dimensions(&webp_vp8_bytes(11, 13), MediaKind::Webp)
            .unwrap()
            .width,
        11
    );
    assert!(MediaPreflight::inspect(b"", None).corrupt);
    assert!(MediaPreflight::inspect(b"\xff\xd8\xff", Some("image/jpeg")).corrupt);
    assert!(MediaPreflight::inspect(b"RIFF\0\0\0\0WEBPxxxx", Some("image/webp")).corrupt);
}

#[tokio::test]
async fn protocol_client_keeps_explicit_headers_and_request_metadata_separate() {
    let _guard = allow_real_model_requests_guard();
    let response_body = json!({
        "id": "chatcmpl_headers",
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
    });
    let http_client = Arc::new(SequenceHttpClient::new(vec![Ok(HttpResponse::ok(
        response_body,
    ))]));
    let client = ProtocolModelClient::new(
        "openai",
        "gpt-test",
        ModelProfile::for_protocol(ProtocolFamily::OpenAiChatCompletions),
        openai_chat_http_config("token"),
        http_client.clone(),
    );
    let mut settings = ModelSettings::default();
    settings.extra_headers.insert(
        "x-request-correlation".to_string(),
        "request_header".to_string(),
    );
    let mut params = ModelRequestParameters::default();
    params.metadata.insert(
        "starweaver.session_id".to_string(),
        json!("session_http_metadata"),
    );
    params.metadata.insert(
        "starweaver.durable_run_id".to_string(),
        json!("run_http_metadata"),
    );
    params
        .metadata
        .insert("cli.session_id".to_string(), json!("session_http_metadata"));
    params
        .metadata
        .insert("cli.run_id".to_string(), json!("run_http_metadata"));

    let response = client
        .request(
            vec![starweaver_model::ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            Some(settings),
            params,
            ModelRequestContext::new(
                RunId::from_string("run_http_header"),
                ConversationId::from_string("conversation_http_header"),
            ),
        )
        .await
        .unwrap();

    assert!(matches!(
        &response.parts[0],
        ModelResponsePart::Text { text } if text == "ok"
    ));
    let requests = http_client.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].headers["x-request-correlation"],
        "request_header"
    );
    assert_eq!(requests[0].headers["authorization"], "Bearer token");
    assert!(!requests[0].headers.contains_key("session_id"));
    assert!(!requests[0].headers.contains_key("session-id"));
    assert!(!requests[0].headers.contains_key("x-client-request-id"));
    assert_eq!(
        requests[0].metadata["starweaver.session_id"],
        "session_http_metadata"
    );
    assert_eq!(
        requests[0].metadata["starweaver.durable_run_id"],
        "run_http_metadata"
    );
    assert_eq!(
        requests[0].metadata["cli.session_id"],
        "session_http_metadata"
    );
    assert_eq!(requests[0].metadata["cli.run_id"], "run_http_metadata");
    assert_eq!(requests[0].metadata["starweaver.run_id"], "run_http_header");
}

#[test]
fn request_context_and_real_request_guard_restore_state() {
    set_allow_real_model_requests(true);
    assert!(allow_real_model_requests());
    {
        let _guard = block_real_model_requests();
        assert!(!allow_real_model_requests());
        {
            let _inner = allow_real_model_requests_guard();
            assert!(allow_real_model_requests());
        }
        assert!(!allow_real_model_requests());
    }
    assert!(allow_real_model_requests());

    let context = ModelRequestContext::new(
        RunId::from_string("run_cov"),
        ConversationId::from_string("conv_cov"),
    )
    .with_llm_trace_metadata(Map::from_iter([("request".to_string(), json!("trace"))]));
    let debug = format!("{context:?}");
    assert!(debug.contains("ModelRequestContext"));
    assert_eq!(context.llm_trace_metadata["request"], "trace");

    let params = ModelRequestParameters::default();
    assert!(params.tools.is_empty());

    let data_url_part = ContentPart::DataUrl {
        data_url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
        media_type: "image/png".to_string(),
    };
    assert_eq!(
        serde_json::to_value(data_url_part).unwrap()["kind"],
        "data_url"
    );
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    bytes.extend_from_slice(&13u32.to_be_bytes());
    bytes.extend_from_slice(b"IHDR");
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&[8, 2, 0, 0, 0]);
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(b"IEND");
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}

fn webp_vp8x_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"RIFF\x1e\0\0\0WEBPVP8X".to_vec();
    bytes.extend_from_slice(&[0; 8]);
    write_24_le(&mut bytes, width - 1);
    write_24_le(&mut bytes, height - 1);
    bytes
}

fn webp_vp8l_bytes(width: u32, height: u32) -> Vec<u8> {
    let packed = ((height - 1) << 14) | ((width - 1) & 0x3fff);
    let mut bytes = b"RIFF\x19\0\0\0WEBPVP8L".to_vec();
    bytes.extend_from_slice(&[0; 5]);
    bytes.extend_from_slice(&[
        (packed & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 24) & 0xff) as u8,
    ]);
    bytes
}

fn webp_vp8_bytes(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = b"RIFF\x1e\0\0\0WEBPVP8 ".to_vec();
    bytes.extend_from_slice(&[0; 7]);
    bytes.extend_from_slice(b"\x9d\x01\x2a");
    bytes.extend_from_slice(&width.to_le_bytes());
    bytes.extend_from_slice(&height.to_le_bytes());
    bytes
}

fn write_24_le(bytes: &mut Vec<u8>, value: u32) {
    bytes.push((value & 0xff) as u8);
    bytes.push(((value >> 8) & 0xff) as u8);
    bytes.push(((value >> 16) & 0xff) as u8);
}

#[derive(Clone)]
#[allow(clippy::unnecessary_literal_bound)]
struct StaticAdapter {
    profile: ModelProfile,
    settings: ModelSettings,
}

#[allow(clippy::unnecessary_literal_bound)]
#[async_trait]
impl ModelAdapter for StaticAdapter {
    fn model_name(&self) -> &str {
        "static-model"
    }

    fn provider_name(&self) -> Option<&str> {
        Some("static-provider")
    }

    fn profile(&self) -> &ModelProfile {
        &self.profile
    }

    fn default_settings(&self) -> Option<&ModelSettings> {
        Some(&self.settings)
    }

    async fn request(
        &self,
        _messages: Vec<starweaver_model::ModelMessage>,
        _settings: Option<ModelSettings>,
        _params: ModelRequestParameters,
        _context: ModelRequestContext,
    ) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse {
            parts: vec![ModelResponsePart::Text {
                text: "static".to_string(),
            }],
            finish_reason: Some(FinishReason::Stop),
            usage: Usage {
                requests: 1,
                input_tokens: 1,
                cache_write_tokens: 0,
                cache_read_tokens: 0,
                output_tokens: 1,
                total_tokens: 2,
                tool_calls: 0,
            },
            model_name: Some("static-model".to_string()),
            provider: None,
            timestamp: None,
            run_id: None,
            conversation_id: None,
            metadata: Map::new(),
        })
    }
}

#[test]
fn core_contracts_are_covered() {
    assert_eq!(sdk_name(), "starweaver-agent-sdk");
    assert_eq!(AgentId::default().as_str(), "main");
    assert_eq!(AgentId::from_string("agent_cov").as_str(), "agent_cov");
    assert_eq!(
        SessionId::from_string("session_cov").as_str(),
        "session_cov"
    );
    assert!(RunId::new().as_str().starts_with("run_"));
    assert!(ConversationId::new().as_str().starts_with("conv_"));
    assert!(CheckpointId::new().as_str().starts_with("ckpt_"));
    assert!(TaskId::new().as_str().starts_with("task_"));

    let mut metadata = Metadata::default();
    metadata.insert("tenant".to_string(), Value::String("acme".to_string()));
    let trace =
        TraceContext::from_trace_parent("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
            .with_span_id("span_cov")
            .with_parent_span_id("parent_cov")
            .with_trace_state("vendor=state")
            .with_metadata(metadata.clone());
    assert_eq!(
        trace.trace_id.as_deref(),
        Some("4bf92f3577b34da6a3ce929d0e0e4736")
    );
    assert_eq!(trace.span_id.as_deref(), Some("span_cov"));
    assert_eq!(trace.parent_span_id.as_deref(), Some("parent_cov"));
    assert_eq!(trace.trace_state.as_deref(), Some("vendor=state"));
    assert_eq!(trace.metadata, metadata);
    assert_eq!(
        TraceContext::from_trace_parent("fallback-trace")
            .trace_id
            .as_deref(),
        Some("fallback-trace")
    );

    let spec = SubagentSpec::new("reviewer", "Review code", "Inspect changes")
        .with_tools(vec!["grep".to_string()])
        .with_optional_tools(vec!["view".to_string()]);
    assert_eq!(spec.tools, ["grep"]);
    assert_eq!(spec.optional_tools, ["view"]);

    let event = SubagentLifecycleEvent::new(
        SubagentLifecycleKind::Started,
        "reviewer",
        TaskId::from_string("task_cov"),
    )
    .with_run_id(RunId::from_string("run_cov_event"))
    .with_metadata(json!({"phase": "start"}));
    assert_eq!(event.kind, SubagentLifecycleKind::Started);
    assert_eq!(
        event.run_id.as_ref().map(RunId::as_str),
        Some("run_cov_event")
    );
    assert_eq!(event.metadata["phase"], "start");

    let mut usage = Usage {
        requests: 1,
        input_tokens: 2,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 3,
        total_tokens: 5,
        tool_calls: 1,
    };
    usage.add_assign(&Usage {
        requests: 2,
        input_tokens: 4,
        cache_write_tokens: 0,
        cache_read_tokens: 0,
        output_tokens: 6,
        total_tokens: 10,
        tool_calls: 3,
    });
    assert_eq!(usage.requests, 3);
    assert_eq!(usage.with_additional_tool_calls(2).tool_calls, 6);
}

#[tokio::test]
async fn model_adapter_defaults_are_covered() {
    let bedrock = ModelProfile::for_protocol(ProtocolFamily::BedrockConverse);
    assert!(bedrock.supports_document_input);
    assert_eq!(
        bedrock.default_structured_output_mode,
        starweaver_model::StructuredOutputMode::Tool
    );

    let adapter = StaticAdapter {
        profile: bedrock,
        settings: ModelSettings::default(),
    };
    assert_eq!(adapter.model_name(), "static-model");
    assert_eq!(adapter.provider_name(), Some("static-provider"));
    assert!(adapter.default_settings().is_some());
    assert!(adapter.profile().supports_tools);

    let events = adapter
        .request_stream(
            Vec::new(),
            None,
            ModelRequestParameters::default(),
            ModelRequestContext::new(
                RunId::from_string("run_adapter"),
                ConversationId::from_string("conv_adapter"),
            ),
        )
        .await
        .unwrap();
    assert!(matches!(
        events.as_slice(),
        [ModelResponseStreamEvent::FinalResult(_)]
    ));

    let mut incremental = adapter
        .request_stream_incremental(
            Vec::new(),
            None,
            ModelRequestParameters::default(),
            ModelRequestContext::new(
                RunId::from_string("run_adapter_stream"),
                ConversationId::from_string("conv_adapter_stream"),
            ),
        )
        .await
        .unwrap();
    assert!(incremental.recv().await.is_some());
    assert!(incremental.recv().await.is_none());

    let counted = adapter
        .count_tokens(&[], None, &ModelRequestParameters::default())
        .await
        .unwrap();
    assert_eq!(counted, Usage::default());
}
