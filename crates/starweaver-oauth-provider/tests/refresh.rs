//! OAuth refresh supervisor tests.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use starweaver_oauth::{OAuthAccount, OAuthError, OAuthResult, OAuthTokenSource, TokenSnapshot};
use starweaver_oauth_provider::{
    oauth_provider_name_from_model, oauth_provider_names_from_models, OAuthRefreshSupervisor,
};
use tokio::sync::Mutex;

struct FakeTokenSource {
    fail: bool,
    refresh_count: Mutex<u64>,
}

impl FakeTokenSource {
    fn new(fail: bool) -> Self {
        Self {
            fail,
            refresh_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl OAuthTokenSource for FakeTokenSource {
    async fn get_token(&self) -> OAuthResult<TokenSnapshot> {
        Ok(TokenSnapshot {
            provider_name: "codex".to_string(),
            access_token: "access".to_string(),
            account: OAuthAccount::default(),
            base_url: None,
            metadata: BTreeMap::new(),
        })
    }

    async fn refresh_token(&self) -> OAuthResult<TokenSnapshot> {
        *self.refresh_count.lock().await += 1;
        if self.fail {
            return Err(OAuthError::InvalidResponse("refresh failed".to_string()));
        }
        self.get_token().await
    }
}

#[test]
fn parses_oauth_provider_names_from_model_strings() {
    assert_eq!(
        oauth_provider_name_from_model(Some("oauth@codex:gpt-5.5")),
        Some("codex".to_string())
    );
    assert_eq!(oauth_provider_name_from_model(Some("openai:gpt-4o")), None);
    assert_eq!(oauth_provider_name_from_model(Some("oauth@codex")), None);
    assert_eq!(oauth_provider_name_from_model(None), None);

    let providers = oauth_provider_names_from_models([
        "oauth@codex:gpt-5.5",
        "openai:gpt-4o",
        "oauth@codex:gpt-5.5",
    ]);
    assert_eq!(providers.into_iter().collect::<Vec<_>>(), vec!["codex"]);
}

#[tokio::test]
async fn refresh_supervisor_records_success_and_failure() {
    let ok_source = Arc::new(FakeTokenSource::new(false));
    let fail_source = Arc::new(FakeTokenSource::new(true));
    let supervisor = OAuthRefreshSupervisor::new(BTreeMap::from([
        (
            "codex".to_string(),
            ok_source.clone() as Arc<dyn OAuthTokenSource>,
        ),
        (
            "broken".to_string(),
            fail_source.clone() as Arc<dyn OAuthTokenSource>,
        ),
    ]));

    let result = supervisor.refresh_once().await;
    let status = supervisor.status().await;

    assert!(result["codex"].is_ok());
    assert!(result["broken"].is_err());
    assert_eq!(*ok_source.refresh_count.lock().await, 1);
    assert_eq!(*fail_source.refresh_count.lock().await, 1);
    assert_eq!(status.providers["codex"].refresh_count, 1);
    assert_eq!(status.providers["codex"].failure_count, 0);
    assert_eq!(status.providers["broken"].refresh_count, 0);
    assert_eq!(status.providers["broken"].failure_count, 1);
    assert_eq!(
        status.providers["broken"].last_error.as_deref(),
        Some("invalid OAuth response: refresh failed")
    );
}

#[tokio::test]
async fn refresh_supervisor_start_and_shutdown() {
    let source = Arc::new(FakeTokenSource::new(false));
    let mut supervisor = OAuthRefreshSupervisor::with_options(
        BTreeMap::from([(
            "codex".to_string(),
            source.clone() as Arc<dyn OAuthTokenSource>,
        )]),
        std::time::Duration::from_mins(1),
        std::time::Duration::from_secs(1),
        true,
    );

    supervisor.start().await;
    for _ in 0..100 {
        if *source.refresh_count.lock().await == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    supervisor.shutdown().await;

    assert_eq!(*source.refresh_count.lock().await, 1);
    assert!(!supervisor.is_running());
}
