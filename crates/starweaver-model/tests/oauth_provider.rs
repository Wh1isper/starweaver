//! OAuth-backed model provider behavior tests.
#![allow(
    clippy::significant_drop_tightening,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use starweaver_core::CancellationToken;
use starweaver_model::{
    build_codex_headers, build_session_headers, patch_codex_responses_body, transport::HttpMethod,
    CodexOAuthResponsesModel, CodexSettings, HttpModelConfig, HttpRequest, HttpResponse,
    ModelAdapter, ModelError, ModelEventStream, ModelHttpClient, ModelRequestContext,
    ModelRequestParameters, ModelSettings, OAuthBearerHttpClient, ProviderSettings,
};
use starweaver_oauth::{OAuthAccount, OAuthResult, OAuthTokenSource, TokenSnapshot};
use tokio::sync::Mutex;

#[derive(Default)]
struct FakeTokenSource {
    refresh_count: Mutex<u64>,
}

#[async_trait]
impl OAuthTokenSource for FakeTokenSource {
    async fn get_token(&self) -> OAuthResult<TokenSnapshot> {
        Ok(TokenSnapshot {
            provider_name: "codex".to_string(),
            access_token: "access-old".to_string(),
            account: OAuthAccount {
                chatgpt_account_id: Some("acct_123".to_string()),
                chatgpt_account_is_fedramp: true,
                ..OAuthAccount::default()
            },
            base_url: None,
            metadata: BTreeMap::new(),
        })
    }

    async fn refresh_token(&self) -> OAuthResult<TokenSnapshot> {
        *self.refresh_count.lock().await += 1;
        Ok(TokenSnapshot {
            provider_name: "codex".to_string(),
            access_token: "access-new".to_string(),
            account: OAuthAccount {
                chatgpt_account_id: Some("acct_456".to_string()),
                ..OAuthAccount::default()
            },
            base_url: None,
            metadata: BTreeMap::new(),
        })
    }
}

#[derive(Default)]
struct RecordingHttpClient {
    seen: Mutex<Vec<HttpRequest>>,
}

#[async_trait]
impl ModelHttpClient for RecordingHttpClient {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, ModelError> {
        let mut seen = self.seen.lock().await;
        seen.push(request);
        if seen.len() == 1 {
            return Err(ModelError::ProviderStatus {
                status: 401,
                body: json!({"error": "unauthorized"}),
                retryable: false,
            });
        }
        Ok(HttpResponse::ok(json!({"ok": true})))
    }

    async fn send_event_stream_incremental(
        &self,
        request: HttpRequest,
    ) -> Result<ModelEventStream, ModelError> {
        self.seen.lock().await.push(request);
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        tokio::spawn(async move {
            for event in [
                json!({
                    "type": "response.created",
                    "response": {"id": "resp_codex_stream", "status": "in_progress"}
                }),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "resp_codex_stream",
                        "status": "completed",
                        "model": "gpt-5.5",
                        "output": [
                            {
                                "type": "message",
                                "content": [{"type": "output_text", "text": "ok"}]
                            }
                        ],
                        "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                    }
                }),
            ] {
                if sender.send(Ok(event)).await.is_err() {
                    return;
                }
            }
        });
        Ok(ModelEventStream::new(receiver))
    }
}

fn codex_request() -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
        headers: BTreeMap::new(),
        body: json!({"model": "gpt-5.5", "instructions": null}),
        timeout: None,
        metadata: serde_json::Map::from_iter([
            (
                "provider.codex.session_id".to_string(),
                json!("provider-session-1"),
            ),
            (
                "provider.codex.thread_id".to_string(),
                json!("provider-thread-1"),
            ),
            (
                "starweaver.durable_session_id".to_string(),
                json!("session-1"),
            ),
            ("starweaver.durable_run_id".to_string(), json!("run-1")),
            ("cli.session_id".to_string(), json!("cli-session-1")),
            ("cli.run_id".to_string(), json!("cli-run-1")),
            (
                "starweaver.conversation_id".to_string(),
                json!("conversation-1"),
            ),
            ("starweaver.run_id".to_string(), json!("runtime-run-1")),
        ]),
        cancellation_token: CancellationToken::default(),
    }
}

#[test]
fn codex_headers_include_account_and_session_metadata() {
    let headers = build_codex_headers(
        &OAuthAccount {
            chatgpt_account_id: Some("acct_123".to_string()),
            chatgpt_account_is_fedramp: true,
            ..OAuthAccount::default()
        },
        Some(&BTreeMap::from([
            ("session_id".to_string(), "s1".to_string()),
            ("thread-id".to_string(), "t1".to_string()),
        ])),
    )
    .unwrap();

    assert_eq!(headers["ChatGPT-Account-ID"], "acct_123");
    assert_eq!(headers["X-OpenAI-Fedramp"], "true");
    assert_eq!(headers["originator"], "starweaver");
    assert!(!headers.contains_key("version"));
    assert_eq!(headers["session_id"], "s1");
    assert_eq!(headers["thread-id"], "t1");
}

#[test]
fn codex_headers_omit_authorization_and_version_by_default() {
    let headers = build_codex_headers(&OAuthAccount::default(), None).unwrap();

    assert_eq!(headers["originator"], "starweaver");
    assert!(!headers.contains_key("Authorization"));
    assert!(!headers.contains_key("version"));
}

#[test]
fn codex_headers_reject_reserved_extra_headers() {
    let error = build_codex_headers(
        &OAuthAccount::default(),
        Some(&BTreeMap::from([(
            "Authorization".to_string(),
            "Bearer other".to_string(),
        )])),
    )
    .unwrap_err();

    assert!(error.to_string().contains("reserved OAuth/Codex header"));
}

#[test]
fn session_headers_use_both_variants() {
    assert_eq!(
        build_session_headers(Some("session"), Some("thread")),
        BTreeMap::from([
            ("session_id".to_string(), "session".to_string()),
            ("session-id".to_string(), "session".to_string()),
            ("thread_id".to_string(), "thread".to_string()),
            ("thread-id".to_string(), "thread".to_string()),
            ("x-client-request-id".to_string(), "thread".to_string()),
        ])
    );
}

#[test]
fn patch_codex_responses_body_fills_required_fields() {
    let mut request = codex_request();

    patch_codex_responses_body(&mut request);

    assert_eq!(request.body["instructions"], "");
    assert_eq!(request.body["store"], false);
}

#[test]
fn patch_codex_responses_body_matches_path_with_query_or_trailing_slash() {
    for url in [
        "https://chatgpt.com/backend-api/codex/responses?attempt=1",
        "https://chatgpt.com/backend-api/codex/responses/?attempt=1",
    ] {
        let mut request = codex_request();
        request.url = url.to_string();
        request.body = json!({"model": "gpt-5.5"});

        patch_codex_responses_body(&mut request);

        assert_eq!(request.body["instructions"], "");
        assert_eq!(request.body["store"], false);
    }
}

#[test]
fn patch_codex_responses_body_matches_instruction_truthiness() {
    let falsy_values = [
        json!(null),
        json!(""),
        json!(false),
        json!(0),
        json!(0.0),
        json!([]),
        json!({}),
    ];
    for value in falsy_values {
        let mut request = codex_request();
        request.body = json!({"model": "gpt-5.5", "instructions": value});

        patch_codex_responses_body(&mut request);

        assert_eq!(request.body["instructions"], "");
        assert_eq!(request.body["store"], false);
    }

    let mut missing = codex_request();
    missing.body = json!({"model": "gpt-5.5"});
    patch_codex_responses_body(&mut missing);
    assert_eq!(missing.body["instructions"], "");
    assert_eq!(missing.body["store"], false);

    let mut non_empty = codex_request();
    non_empty.body = json!({"model": "gpt-5.5", "instructions": "keep", "store": true});
    patch_codex_responses_body(&mut non_empty);
    assert_eq!(non_empty.body["instructions"], "keep");
    assert_eq!(non_empty.body["store"], false);
}

#[tokio::test]
async fn codex_oauth_streaming_model_builds_subscription_request_shape() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let model = CodexOAuthResponsesModel::with_http_client(
        "gpt-5.5",
        HttpModelConfig::new("https://chatgpt.com/backend-api/codex", "responses"),
        token_source,
        BTreeMap::new(),
        http_client.clone(),
    )
    .unwrap();

    let events = model
        .request_stream(
            vec![starweaver_model::ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            Some(ModelSettings {
                provider_settings: ProviderSettings {
                    codex: Some(CodexSettings {
                        session_id: Some("typed-session".to_string()),
                        thread_id: Some("typed-thread".to_string()),
                    }),
                    ..ProviderSettings::default()
                },
                ..ModelSettings::default()
            }),
            ModelRequestParameters::default(),
            ModelRequestContext::new(
                starweaver_core::RunId::from_string("run_codex_stream"),
                starweaver_core::ConversationId::from_string("conv_codex_stream"),
            ),
        )
        .await
        .unwrap();

    assert!(events.iter().any(|event| matches!(
        event,
        starweaver_model::ModelResponseStreamEvent::FinalResult(response)
            if response.text_output() == "ok"
    )));
    let seen = http_client.seen.lock().await;
    assert_eq!(seen.len(), 1);
    let request = &seen[0];
    assert_eq!(
        request.url,
        "https://chatgpt.com/backend-api/codex/responses"
    );
    assert_eq!(request.headers["Authorization"], "Bearer access-old");
    assert_eq!(request.headers["originator"], "starweaver");
    assert!(request.headers["User-Agent"].starts_with("starweaver-agent-sdk/"));
    assert_eq!(request.headers["ChatGPT-Account-ID"], "acct_123");
    assert_eq!(request.headers["X-OpenAI-Fedramp"], "true");
    assert_eq!(request.headers["session_id"], "typed-session");
    assert_eq!(request.headers["session-id"], "typed-session");
    assert_eq!(request.headers["thread_id"], "typed-thread");
    assert_eq!(request.headers["thread-id"], "typed-thread");
    assert_eq!(request.headers["x-client-request-id"], "typed-thread");
    assert!(!request.headers.contains_key("version"));
    assert_eq!(request.body["model"], "gpt-5.5");
    assert_eq!(request.body["stream"], true);
    assert_eq!(request.body["instructions"], "");
    assert_eq!(request.body["store"], false);
    assert_eq!(request.body["input"][0]["content"][0]["text"], "hello");
    assert_eq!(
        request.metadata["provider.codex.session_id"],
        "typed-session"
    );
    assert_eq!(request.metadata["provider.codex.thread_id"], "typed-thread");
}

#[tokio::test]
async fn codex_oauth_headers_prefer_provider_specific_metadata() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let client =
        OAuthBearerHttpClient::new(http_client.clone(), token_source, "codex", BTreeMap::new())
            .unwrap();
    let mut request = codex_request();
    request.metadata.insert(
        "provider.codex.session_id".to_string(),
        json!("codex-session"),
    );
    request.metadata.insert(
        "provider.codex.thread_id".to_string(),
        json!("codex-thread"),
    );

    let _ = client.send(request).await;

    let seen = http_client.seen.lock().await;
    assert_eq!(seen[0].headers["session_id"], "codex-session");
    assert_eq!(seen[0].headers["session-id"], "codex-session");
    assert_eq!(seen[0].headers["thread_id"], "codex-thread");
    assert_eq!(seen[0].headers["thread-id"], "codex-thread");
    assert_eq!(seen[0].headers["x-client-request-id"], "codex-thread");
}

#[tokio::test]
async fn codex_oauth_preserves_explicit_request_routing_headers_over_metadata() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let client =
        OAuthBearerHttpClient::new(http_client.clone(), token_source, "codex", BTreeMap::new())
            .unwrap();
    let mut request = codex_request();
    request.headers.extend(BTreeMap::from([
        (
            "session_id".to_string(),
            "explicit-session-underscore".to_string(),
        ),
        (
            "session-id".to_string(),
            "explicit-session-hyphen".to_string(),
        ),
        (
            "thread_id".to_string(),
            "explicit-thread-underscore".to_string(),
        ),
        (
            "thread-id".to_string(),
            "explicit-thread-hyphen".to_string(),
        ),
        (
            "x-client-request-id".to_string(),
            "explicit-client-request".to_string(),
        ),
    ]));

    let _ = client.send(request).await;

    let seen = http_client.seen.lock().await;
    assert_eq!(seen[0].headers["session_id"], "explicit-session-underscore");
    assert_eq!(seen[0].headers["session-id"], "explicit-session-hyphen");
    assert_eq!(seen[0].headers["thread_id"], "explicit-thread-underscore");
    assert_eq!(seen[0].headers["thread-id"], "explicit-thread-hyphen");
    assert_eq!(
        seen[0].headers["x-client-request-id"],
        "explicit-client-request"
    );
}

#[tokio::test]
async fn codex_oauth_single_explicit_alias_removes_generated_alias_group() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let client =
        OAuthBearerHttpClient::new(http_client.clone(), token_source, "codex", BTreeMap::new())
            .unwrap();
    let mut request = codex_request();
    request.headers.insert(
        "session_id".to_string(),
        "explicit-session-only".to_string(),
    );
    request
        .headers
        .insert("thread-id".to_string(), "explicit-thread-only".to_string());
    request.metadata.insert(
        "provider.codex.session_id".to_string(),
        json!("metadata-session"),
    );
    request.metadata.insert(
        "provider.codex.thread_id".to_string(),
        json!("metadata-thread"),
    );

    let _ = client.send(request).await;

    let seen = http_client.seen.lock().await;
    assert_eq!(seen[0].headers["session_id"], "explicit-session-only");
    assert!(!seen[0].headers.contains_key("session-id"));
    assert_eq!(seen[0].headers["thread-id"], "explicit-thread-only");
    assert!(!seen[0].headers.contains_key("thread_id"));
    assert!(!seen[0].headers.contains_key("x-client-request-id"));
}

#[tokio::test]
async fn codex_oauth_extra_header_alias_removes_generated_alias_group() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let client = OAuthBearerHttpClient::new(
        http_client.clone(),
        token_source,
        "codex",
        BTreeMap::from([
            ("Session-ID".to_string(), "extra-session-only".to_string()),
            (
                "X-CLIENT-REQUEST-ID".to_string(),
                "extra-thread-only".to_string(),
            ),
        ]),
    )
    .unwrap();

    let _ = client.send(codex_request()).await;

    let seen = http_client.seen.lock().await;
    assert_eq!(seen[0].headers["Session-ID"], "extra-session-only");
    assert!(!seen[0].headers.contains_key("session_id"));
    assert!(!seen[0].headers.contains_key("session-id"));
    assert_eq!(seen[0].headers["X-CLIENT-REQUEST-ID"], "extra-thread-only");
    assert!(!seen[0].headers.contains_key("thread_id"));
    assert!(!seen[0].headers.contains_key("thread-id"));
    assert!(!seen[0].headers.contains_key("x-client-request-id"));
}

#[tokio::test]
async fn oauth_bearer_http_client_refreshes_once_on_401() {
    let token_source = Arc::new(FakeTokenSource::default());
    let http_client = Arc::new(RecordingHttpClient::default());
    let client = OAuthBearerHttpClient::new(
        http_client.clone(),
        token_source.clone(),
        "codex",
        BTreeMap::new(),
    )
    .unwrap();

    let response = client.send(codex_request()).await.unwrap();

    assert_eq!(response.body["ok"], true);
    assert_eq!(*token_source.refresh_count.lock().await, 1);
    let seen = http_client.seen.lock().await;
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].headers["Authorization"], "Bearer access-old");
    assert_eq!(seen[1].headers["Authorization"], "Bearer access-new");
    assert_eq!(seen[0].headers["ChatGPT-Account-ID"], "acct_123");
    assert_eq!(seen[0].headers["X-OpenAI-Fedramp"], "true");
    assert_eq!(seen[1].headers["ChatGPT-Account-ID"], "acct_456");
    assert!(!seen[1].headers.contains_key("X-OpenAI-Fedramp"));
    assert_eq!(seen[0].headers["originator"], "starweaver");
    assert!(!seen[0].headers.contains_key("version"));
    assert!(seen[0].headers["User-Agent"].starts_with("starweaver-agent-sdk/"));
    assert_eq!(seen[0].body["instructions"], "");
    assert_eq!(seen[0].body["store"], false);
    assert_eq!(seen[0].headers["session_id"], "provider-session-1");
    assert_eq!(seen[0].headers["session-id"], "provider-session-1");
    assert_eq!(seen[0].headers["thread_id"], "provider-thread-1");
    assert_eq!(seen[0].headers["thread-id"], "provider-thread-1");
    assert_eq!(seen[0].headers["x-client-request-id"], "provider-thread-1");
}

#[test]
fn codex_model_builder_rejects_non_stream_request() {
    let token_source = Arc::new(FakeTokenSource::default());
    let model = starweaver_model::build_codex_model(
        "gpt-5.5",
        token_source,
        HttpModelConfig::new("https://chatgpt.com/backend-api/codex", "responses"),
        BTreeMap::new(),
    )
    .unwrap();

    let runtime = tokio::runtime::Runtime::new().unwrap();
    let error = runtime
        .block_on(model.request(
            vec![starweaver_model::ModelMessage::Request(
                starweaver_model::ModelRequest::user_text("hello"),
            )],
            None,
            ModelRequestParameters::default(),
            ModelRequestContext::new(
                starweaver_core::RunId::new(),
                starweaver_core::ConversationId::new(),
            ),
        ))
        .unwrap_err();

    assert!(error.to_string().contains("requires streaming"));
}
