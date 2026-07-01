//! OAuth store and Codex token metadata tests.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use serde_json::json;
use starweaver_oauth::{
    AuthFile, CODEX_BASE_URL, CODEX_CLIENT_ID, CODEX_ISSUER, CODEX_TOKEN_ENDPOINT,
    CodexOAuthClient, CodexOAuthProfile, OAuthAccount, OAuthProviderRecord, OAuthStore,
    OAuthTokens, account_from_id_token,
};

fn fixture_record() -> OAuthProviderRecord {
    OAuthProviderRecord {
        provider_type: "oauth2".to_string(),
        issuer: CODEX_ISSUER.to_string(),
        client_id: CODEX_CLIENT_ID.to_string(),
        token_endpoint: CODEX_TOKEN_ENDPOINT.to_string(),
        revoke_endpoint: None,
        base_url: Some(CODEX_BASE_URL.to_string()),
        scopes: vec!["openid".to_string()],
        tokens: OAuthTokens {
            id_token: Some("id-old".to_string()),
            access_token: "access-old".to_string(),
            refresh_token: Some("refresh-old".to_string()),
        },
        account: OAuthAccount {
            email: Some("user@example.com".to_string()),
            chatgpt_user_id: Some("user_123".to_string()),
            chatgpt_account_id: Some("acct_123".to_string()),
            chatgpt_plan_type: Some("plus".to_string()),
            chatgpt_account_is_fedramp: false,
        },
        last_refresh_at: Some(Utc::now()),
    }
}

#[test]
fn oauth_store_round_trips_provider_records() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let path = temp.path().join("auth.json");
    let store = OAuthStore::new(path.clone());
    let record = fixture_record();

    store
        .set_provider("codex", record.clone())
        .expect("provider should be saved");

    assert_eq!(store.get_provider("codex").unwrap(), Some(record));
    assert_eq!(store.load().unwrap().version, AuthFile::default().version);
    assert!(path.exists());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let dir_mode = temp.path().metadata().unwrap().permissions().mode() & 0o777;
        let file_mode = path.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    assert!(store.remove_provider("codex").unwrap());
    assert!(store.get_provider("codex").unwrap().is_none());
}

#[test]
fn refreshed_tokens_preserve_fields_codex_omits() {
    let record = fixture_record();

    let refreshed = record.with_refreshed_tokens(
        None,
        Some("access-new".to_string()),
        None,
        Some(OAuthAccount {
            email: Some("user@example.com".to_string()),
            chatgpt_user_id: Some("user_123".to_string()),
            chatgpt_account_id: Some("acct_123".to_string()),
            chatgpt_plan_type: Some("team".to_string()),
            chatgpt_account_is_fedramp: true,
        }),
    );

    assert_eq!(refreshed.tokens.id_token.as_deref(), Some("id-old"));
    assert_eq!(refreshed.tokens.access_token, "access-new");
    assert_eq!(
        refreshed.tokens.refresh_token.as_deref(),
        Some("refresh-old")
    );
    assert_eq!(
        refreshed.account.chatgpt_account_id.as_deref(),
        Some("acct_123")
    );
    assert!(refreshed.account.chatgpt_account_is_fedramp);
    assert!(refreshed.last_refresh_at.is_some());
}

#[test]
fn account_from_id_token_extracts_codex_claims() {
    let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "email": "top@example.com",
            "https://api.openai.com/profile": {"email": "profile@example.com"},
            "https://api.openai.com/auth": {
                "chatgpt_user_id": "user_123",
                "chatgpt_account_id": "acct_123",
                "chatgpt_plan_type": {"raw_value": "enterprise"},
                "chatgpt_account_is_fedramp": true
            }
        }))
        .unwrap(),
    );
    let token = format!("{header}.{payload}.signature");

    let account = account_from_id_token(&token).unwrap();

    assert_eq!(account.email.as_deref(), Some("top@example.com"));
    assert_eq!(account.chatgpt_user_id.as_deref(), Some("user_123"));
    assert_eq!(account.chatgpt_account_id.as_deref(), Some("acct_123"));
    assert_eq!(account.chatgpt_plan_type.as_deref(), Some("enterprise"));
    assert!(account.chatgpt_account_is_fedramp);
}

#[tokio::test]
async fn codex_device_login_sends_single_content_type_per_request() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server = TestOAuthServer::spawn(Arc::clone(&requests));
    let temp = tempfile::tempdir().unwrap();
    let profile = CodexOAuthProfile {
        issuer: server.base_url.clone(),
        token_endpoint: format!("{}/oauth/token", server.base_url),
        revoke_endpoint: format!("{}/oauth/revoke", server.base_url),
        base_url: "https://example.test/codex".to_string(),
        ..CodexOAuthProfile::default()
    };
    let client = CodexOAuthClient::with_http_client(
        profile,
        OAuthStore::new(temp.path().join("auth.json")),
        reqwest::Client::new(),
    );

    let (device_code, record) = client.login_device_code(1).await.unwrap();

    assert_eq!(device_code.user_code, "ABCD");
    assert_eq!(record.tokens.access_token, "access-new");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_request(
        &requests[0],
        "/api/accounts/deviceauth/usercode",
        "application/json",
        r#"{"client_id":"app_EMoamEEZ73f0CkXaXp7hrann"}"#,
    );
    assert_request(
        &requests[1],
        "/api/accounts/deviceauth/token",
        "application/json",
        r#"{"device_auth_id":"dev_1","user_code":"ABCD"}"#,
    );
    assert_request(
        &requests[2],
        "/oauth/token",
        "application/x-www-form-urlencoded",
        "grant_type=authorization_code",
    );
}

struct TestOAuthServer {
    base_url: String,
    handle: Option<thread::JoinHandle<()>>,
}

impl TestOAuthServer {
    fn spawn(requests: Arc<Mutex<Vec<String>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(false).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_http_request(&mut stream);
                requests.lock().unwrap().push(request.clone());
                let response_body = match index {
                    0 => r#"{"device_auth_id":"dev_1","user_code":"ABCD","interval":"1"}"#,
                    1 => {
                        r#"{"authorization_code":"auth-code","code_challenge":"challenge","code_verifier":"verifier"}"#
                    }
                    _ => r#"{"access_token":"access-new","refresh_token":"refresh-new"}"#,
                };
                write_json_response(&mut stream, response_body);
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            handle: Some(handle),
        }
    }
}

impl Drop for TestOAuthServer {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert_ne!(read, 0, "client closed request before sending headers");
        bytes.extend_from_slice(&buffer[..read]);
        let Some(header_end) = find_header_end(&bytes) else {
            continue;
        };
        let headers = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.eq_ignore_ascii_case("content-length"))
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        let total = header_end + content_length;
        while bytes.len() < total {
            let read = stream.read(&mut buffer).unwrap();
            assert_ne!(read, 0, "client closed request before sending full body");
            bytes.extend_from_slice(&buffer[..read]);
        }
        return String::from_utf8(bytes[..total].to_vec()).unwrap();
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| position + 4)
}

fn write_json_response(stream: &mut TcpStream, body: &str) {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
    stream.flush().unwrap();
}

fn assert_request(request: &str, path: &str, content_type: &str, body_fragment: &str) {
    assert!(
        request.starts_with(&format!("POST {path} HTTP/1.1\r\n")),
        "unexpected request path: {request}"
    );
    let content_type_count = request
        .lines()
        .filter(|line| line.to_ascii_lowercase().starts_with("content-type:"))
        .count();
    assert_eq!(
        content_type_count, 1,
        "request should include exactly one Content-Type header: {request}"
    );
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case(&format!("content-type: {content_type}"))),
        "request missing expected Content-Type {content_type}: {request}"
    );
    assert!(
        request.contains(body_fragment),
        "request missing expected body fragment {body_fragment}: {request}"
    );
}
