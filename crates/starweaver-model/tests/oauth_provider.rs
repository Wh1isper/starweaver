//! OAuth-backed model provider behavior tests.
#![allow(
    clippy::significant_drop_tightening,
    clippy::unwrap_used,
    clippy::expect_used
)]

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde_json::json;
use starweaver_model::{
    build_codex_headers, build_session_headers, patch_codex_responses_body, transport::HttpMethod,
    HttpModelConfig, HttpRequest, HttpResponse, ModelAdapter, ModelError, ModelHttpClient,
    ModelRequestContext, ModelRequestParameters, OAuthBearerHttpClient,
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
}

fn codex_request() -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Post,
        url: "https://chatgpt.com/backend-api/codex/responses".to_string(),
        headers: BTreeMap::new(),
        body: json!({"model": "gpt-5.5", "instructions": null}),
        timeout: None,
        metadata: serde_json::Map::from_iter([
            ("starweaver.conversation_id".to_string(), json!("session-1")),
            ("starweaver.run_id".to_string(), json!("run-1")),
        ]),
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
fn patch_codex_responses_body_matches_ya_mono_instruction_truthiness() {
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
    assert_eq!(seen[0].headers["session_id"], "session-1");
    assert_eq!(seen[0].headers["session-id"], "session-1");
    assert_eq!(seen[0].headers["thread_id"], "run-1");
    assert_eq!(seen[0].headers["thread-id"], "run-1");
    assert_eq!(seen[0].headers["x-client-request-id"], "run-1");
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
