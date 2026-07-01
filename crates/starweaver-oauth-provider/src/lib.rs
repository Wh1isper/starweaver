#![allow(clippy::missing_errors_doc)]
#![allow(clippy::module_name_repetitions)]
//! OAuth-backed provider helpers for Starweaver.
//!
//! It provides Codex OAuth-backed model helpers and a refresh supervisor for
//! keeping store-backed OAuth token sources warm.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_model::{
    CodexOAuthResponsesModel, HttpModelConfig, MaxTokensParameter, ModelError,
    build_codex_model as build_model_codex_model,
};
use starweaver_oauth::{
    CODEX_BASE_URL, OAuthError, OAuthStore, OAuthTokenSource, TokenSnapshot,
    create_codex_token_source,
};
use tokio::task::JoinHandle;

/// Build a Codex OAuth-backed `OpenAI` Responses model using the default auth store.
///
/// # Errors
///
/// Returns an error when the OAuth token source or model HTTP client cannot be built.
pub fn build_codex_model(
    model_name: impl Into<String>,
) -> Result<CodexOAuthResponsesModel, ModelError> {
    build_codex_model_with_store(model_name, OAuthStore::default_store(), BTreeMap::new())
}

/// Build a Codex OAuth-backed model with an explicit OAuth store and extra headers.
///
/// # Errors
///
/// Returns an error when the OAuth token source or model HTTP client cannot be built.
pub fn build_codex_model_with_store(
    model_name: impl Into<String>,
    store: OAuthStore,
    extra_headers: BTreeMap<String, String>,
) -> Result<CodexOAuthResponsesModel, ModelError> {
    let token_source = create_codex_token_source(Some(store))
        .map_err(|error| ModelError::Transport(error.to_string()))?;
    let mut http_config = HttpModelConfig::new(CODEX_BASE_URL, "responses");
    http_config.max_tokens_parameter = MaxTokensParameter::Omit;
    build_model_codex_model(
        model_name,
        Arc::new(token_source),
        http_config,
        extra_headers,
    )
}

/// Infer an OAuth-backed model from `oauth@provider:model` parts.
///
/// # Errors
///
/// Returns an error for unknown OAuth providers or model construction failures.
pub fn infer_oauth_model(
    provider_name: &str,
    model_name: &str,
) -> Result<CodexOAuthResponsesModel, ModelError> {
    match provider_name {
        "codex" => build_codex_model(model_name),
        other => Err(ModelError::Transport(format!(
            "unknown OAuth provider: {other}"
        ))),
    }
}

/// Return the OAuth provider name from a model string such as `oauth@codex:gpt-5.5`.
#[must_use]
pub fn oauth_provider_name_from_model(model: Option<&str>) -> Option<String> {
    let model = model?;
    let rest = model.strip_prefix("oauth@")?;
    let (provider_name, model_name) = rest.split_once(':')?;
    if provider_name.is_empty() || model_name.is_empty() {
        None
    } else {
        Some(provider_name.to_string())
    }
}

/// Return all OAuth provider names referenced by model strings.
pub fn oauth_provider_names_from_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
) -> BTreeSet<String> {
    let mut providers = BTreeSet::new();
    for model in models {
        if let Some(provider_name) = oauth_provider_name_from_model(Some(model)) {
            providers.insert(provider_name);
        }
    }
    providers
}

/// Per-provider refresh status.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthRefreshProviderStatus {
    /// Provider name.
    pub provider_name: String,
    /// Successful refresh count.
    pub refresh_count: u64,
    /// Failed refresh count.
    pub failure_count: u64,
    /// Last success timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<DateTime<Utc>>,
    /// Last failure timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<DateTime<Utc>>,
    /// Last error string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// Refresh supervisor status snapshot.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthRefreshSupervisorStatus {
    /// Whether the supervisor task is running.
    pub running: bool,
    /// Number of providers managed by this supervisor.
    pub provider_count: usize,
    /// Provider statuses keyed by provider name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<String, OAuthRefreshProviderStatus>,
}

/// Periodically refresh OAuth token sources in the background.
pub struct OAuthRefreshSupervisor {
    token_sources: BTreeMap<String, Arc<dyn OAuthTokenSource>>,
    interval: Duration,
    failure_retry: Duration,
    refresh_on_startup: bool,
    status: Arc<tokio::sync::Mutex<OAuthRefreshSupervisorStatus>>,
    stop: Arc<tokio::sync::Notify>,
    task: Option<JoinHandle<()>>,
}

impl OAuthRefreshSupervisor {
    /// Create a refresh supervisor.
    #[must_use]
    pub fn new(token_sources: BTreeMap<String, Arc<dyn OAuthTokenSource>>) -> Self {
        Self::with_options(
            token_sources,
            Duration::from_mins(30),
            Duration::from_mins(1),
            true,
        )
    }

    /// Create a refresh supervisor with explicit intervals.
    ///
    /// # Panics
    ///
    /// Panics when `interval` or `failure_retry` is zero.
    #[must_use]
    pub fn with_options(
        token_sources: BTreeMap<String, Arc<dyn OAuthTokenSource>>,
        interval: Duration,
        failure_retry: Duration,
        refresh_on_startup: bool,
    ) -> Self {
        assert!(!interval.is_zero(), "interval must be positive");
        assert!(
            !failure_retry.is_zero(),
            "failure retry interval must be positive"
        );
        let providers = token_sources
            .keys()
            .map(|provider_name| {
                (
                    provider_name.clone(),
                    OAuthRefreshProviderStatus {
                        provider_name: provider_name.clone(),
                        ..OAuthRefreshProviderStatus::default()
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        Self {
            token_sources,
            interval,
            failure_retry,
            refresh_on_startup,
            status: Arc::new(tokio::sync::Mutex::new(OAuthRefreshSupervisorStatus {
                running: false,
                provider_count: providers.len(),
                providers,
            })),
            stop: Arc::new(tokio::sync::Notify::new()),
            task: None,
        }
    }

    /// Return whether the supervisor background task is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.task.as_ref().is_some_and(|task| !task.is_finished())
    }

    /// Return provider names managed by this supervisor.
    #[must_use]
    pub fn provider_names(&self) -> Vec<String> {
        self.token_sources.keys().cloned().collect()
    }

    /// Return a status snapshot.
    pub async fn status(&self) -> OAuthRefreshSupervisorStatus {
        let mut status = self.status.lock().await.clone();
        status.running = self.is_running();
        status
    }

    /// Start the background refresh loop.
    pub async fn start(&mut self) {
        if self.is_running() {
            return;
        }
        self.stop = Arc::new(tokio::sync::Notify::new());
        {
            let mut status = self.status.lock().await;
            status.running = true;
        }
        let token_sources = self.token_sources.clone();
        let interval = self.interval;
        let failure_retry = self.failure_retry;
        let refresh_on_startup = self.refresh_on_startup;
        let status = Arc::clone(&self.status);
        let stop = Arc::clone(&self.stop);
        self.task = Some(tokio::spawn(async move {
            if refresh_on_startup {
                tokio::select! {
                    () = stop.notified() => {
                        let mut status = status.lock().await;
                        status.running = false;
                        drop(status);
                        return;
                    }
                    _ = refresh_once_inner(&token_sources, &status) => {}
                }
            }
            loop {
                let sleep_for = next_sleep_seconds(&status, interval, failure_retry).await;
                tokio::select! {
                    () = stop.notified() => break,
                    () = tokio::time::sleep(sleep_for) => {}
                }
                tokio::select! {
                    () = stop.notified() => break,
                    _ = refresh_once_inner(&token_sources, &status) => {}
                }
            }
            let mut status = status.lock().await;
            status.running = false;
        }));
    }

    /// Stop the background refresh loop.
    pub async fn shutdown(&mut self) {
        let Some(task) = self.task.take() else {
            return;
        };
        self.stop.notify_waiters();
        let _ = task.await;
        let mut status = self.status.lock().await;
        status.running = false;
    }

    /// Refresh all providers once.
    pub async fn refresh_once(&self) -> BTreeMap<String, Result<TokenSnapshot, OAuthError>> {
        refresh_once_inner(&self.token_sources, &self.status).await
    }
}

async fn refresh_once_inner(
    token_sources: &BTreeMap<String, Arc<dyn OAuthTokenSource>>,
    status: &Arc<tokio::sync::Mutex<OAuthRefreshSupervisorStatus>>,
) -> BTreeMap<String, Result<TokenSnapshot, OAuthError>> {
    let mut results = BTreeMap::new();
    for (provider_name, token_source) in token_sources {
        match token_source.refresh_token().await {
            Ok(snapshot) => {
                record_success(status, provider_name).await;
                results.insert(provider_name.clone(), Ok(snapshot));
            }
            Err(error) => {
                record_failure(status, provider_name, &error).await;
                results.insert(provider_name.clone(), Err(error));
            }
        }
    }
    results
}

async fn record_success(
    status: &Arc<tokio::sync::Mutex<OAuthRefreshSupervisorStatus>>,
    provider_name: &str,
) {
    let mut status = status.lock().await;
    if let Some(provider) = status.providers.get_mut(provider_name) {
        provider.refresh_count += 1;
        provider.last_success_at = Some(Utc::now());
        provider.last_error = None;
    }
}

async fn record_failure(
    status: &Arc<tokio::sync::Mutex<OAuthRefreshSupervisorStatus>>,
    provider_name: &str,
    error: &OAuthError,
) {
    let mut status = status.lock().await;
    if let Some(provider) = status.providers.get_mut(provider_name) {
        provider.failure_count += 1;
        provider.last_failure_at = Some(Utc::now());
        provider.last_error = Some(error.to_string());
    }
}

async fn next_sleep_seconds(
    status: &Arc<tokio::sync::Mutex<OAuthRefreshSupervisorStatus>>,
    interval: Duration,
    failure_retry: Duration,
) -> Duration {
    let status = status.lock().await;
    if status.providers.values().any(last_attempt_failed) {
        failure_retry
    } else {
        interval
    }
}

fn last_attempt_failed(status: &OAuthRefreshProviderStatus) -> bool {
    match (status.last_failure_at, status.last_success_at) {
        (None, _) => false,
        (Some(_), None) => true,
        (Some(failure), Some(success)) => failure > success,
    }
}

/// Create an OAuth refresh supervisor for model strings.
///
/// # Errors
///
/// Returns an error when a provider token source cannot be created.
pub fn create_oauth_refresh_supervisor_for_models<'a>(
    models: impl IntoIterator<Item = &'a str>,
) -> Result<Option<OAuthRefreshSupervisor>, OAuthError> {
    create_oauth_refresh_supervisor_for_models_with_options(
        models,
        Duration::from_mins(30),
        Duration::from_mins(1),
        true,
    )
}

/// Create an OAuth refresh supervisor for model strings with explicit intervals.
///
/// # Errors
///
/// Returns an error when a provider token source cannot be created.
pub fn create_oauth_refresh_supervisor_for_models_with_options<'a>(
    models: impl IntoIterator<Item = &'a str>,
    interval: Duration,
    failure_retry: Duration,
    refresh_on_startup: bool,
) -> Result<Option<OAuthRefreshSupervisor>, OAuthError> {
    let mut token_sources: BTreeMap<String, Arc<dyn OAuthTokenSource>> = BTreeMap::new();
    for provider_name in oauth_provider_names_from_models(models) {
        if provider_name == "codex" {
            token_sources.insert(
                provider_name.clone(),
                Arc::new(create_codex_token_source(None)?) as Arc<dyn OAuthTokenSource>,
            );
        }
    }
    if token_sources.is_empty() {
        Ok(None)
    } else {
        Ok(Some(OAuthRefreshSupervisor::with_options(
            token_sources,
            interval,
            failure_retry,
            refresh_on_startup,
        )))
    }
}

/// Codex helper namespace.
pub mod codex {
    pub use starweaver_model::{
        CodexOAuthResponsesModel, build_session_headers, codex_model_profile,
    };

    pub use crate::{build_codex_model, build_codex_model_with_store, infer_oauth_model};
}

/// HTTP helper namespace.
pub mod http {
    pub use starweaver_model::{
        CODEX_ORIGINATOR, OAuthBearerHttpClient, build_codex_headers, patch_codex_responses_body,
    };
}

/// Refresh helper namespace.
pub mod refresh {
    pub use crate::{
        OAuthRefreshProviderStatus, OAuthRefreshSupervisor, OAuthRefreshSupervisorStatus,
        create_oauth_refresh_supervisor_for_models,
        create_oauth_refresh_supervisor_for_models_with_options, oauth_provider_name_from_model,
        oauth_provider_names_from_models,
    };
}

pub use starweaver_model::{
    CODEX_ORIGINATOR, OAuthBearerHttpClient, build_codex_headers, build_session_headers,
    codex_model_profile, patch_codex_responses_body,
};
