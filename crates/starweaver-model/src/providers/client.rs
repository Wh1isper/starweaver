//! Production protocol clients built on replay-validated wire mappers.

mod adapter_impl;
mod factory;
mod output_schema;
mod request_options;
mod wire;

use crate::{
    profile::ModelProfile,
    settings::ModelSettings,
    transport::{
        DynHttpClient, DynProviderRequestAuditRecorder, DynSleeper, HttpModelConfig,
        ProviderRequestAuditCapture, ProviderRequestAuditPolicy, TokioSleeper,
    },
};

/// Shared production model client for a supported wire protocol family.
pub struct ProtocolModelClient {
    provider_name: String,
    model_name: String,
    profile: ModelProfile,
    default_settings: Option<ModelSettings>,
    http_config: HttpModelConfig,
    http_client: DynHttpClient,
    sleeper: DynSleeper,
    request_audit: Option<ProviderRequestAuditCapture>,
}

impl ProtocolModelClient {
    /// Create a protocol client with an injected HTTP client.
    #[must_use]
    pub fn new(
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        profile: ModelProfile,
        http_config: HttpModelConfig,
        http_client: DynHttpClient,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            profile,
            default_settings: None,
            http_config,
            http_client,
            sleeper: std::sync::Arc::new(TokioSleeper),
            request_audit: None,
        }
    }

    /// Set adapter-level default settings.
    #[must_use]
    pub fn with_default_settings(mut self, settings: ModelSettings) -> Self {
        self.default_settings = Some(settings);
        self
    }

    /// Override the model capability profile.
    #[must_use]
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = profile;
        self
    }

    /// Set a custom sleeper for retry policy execution.
    #[must_use]
    pub fn with_sleeper(mut self, sleeper: DynSleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    /// Record provider HTTP request audit snapshots outside redacted trace events.
    #[must_use]
    pub fn with_provider_request_audit(
        mut self,
        recorder: DynProviderRequestAuditRecorder,
        policy: ProviderRequestAuditPolicy,
    ) -> Self {
        self.request_audit = Some(ProviderRequestAuditCapture::new(recorder, policy));
        self
    }
}
