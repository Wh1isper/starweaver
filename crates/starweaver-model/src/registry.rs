//! Provider alias registry for protocol family resolution.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    profile::{ModelProfile, ProtocolFamily},
    providers::client::ProtocolModelClient,
    settings::ModelSettings,
    transport::{DynHttpClient, HttpModelConfig, ReqwestHttpClient},
    ModelError,
};

/// Provider alias definition resolved into a protocol client.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProviderAlias {
    /// Alias name used by applications.
    pub alias: String,
    /// Provider name attached to responses and diagnostics.
    pub provider_name: String,
    /// Model name sent to the provider.
    pub model_name: String,
    /// Protocol family used by this alias.
    pub protocol: ProtocolFamily,
    /// HTTP configuration.
    pub http: HttpModelConfig,
    /// Optional profile override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ModelProfile>,
    /// Optional default settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_settings: Option<ModelSettings>,
}

impl ProviderAlias {
    /// Create a provider alias.
    #[must_use]
    pub fn new(
        alias: impl Into<String>,
        provider_name: impl Into<String>,
        model_name: impl Into<String>,
        protocol: ProtocolFamily,
        http: HttpModelConfig,
    ) -> Self {
        Self {
            alias: alias.into(),
            provider_name: provider_name.into(),
            model_name: model_name.into(),
            protocol,
            http,
            profile: None,
            default_settings: None,
        }
    }

    /// Set profile override.
    #[must_use]
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Set default settings.
    #[must_use]
    pub fn with_default_settings(mut self, settings: ModelSettings) -> Self {
        self.default_settings = Some(settings);
        self
    }

    /// Build a protocol model client with an injected HTTP client.
    #[must_use]
    pub fn build_with_client(&self, client: DynHttpClient) -> ProtocolModelClient {
        let profile = self
            .profile
            .clone()
            .unwrap_or_else(|| ModelProfile::for_protocol(self.protocol));
        let model = ProtocolModelClient::new(
            self.provider_name.clone(),
            self.model_name.clone(),
            profile,
            self.http.clone(),
            client,
        );
        if let Some(settings) = &self.default_settings {
            model.with_default_settings(settings.clone())
        } else {
            model
        }
    }

    /// Build a protocol model client with the default HTTP client.
    ///
    /// # Errors
    ///
    /// Returns an error when the default HTTP client cannot be created.
    pub fn build(&self) -> Result<ProtocolModelClient, ModelError> {
        Ok(self.build_with_client(Arc::new(ReqwestHttpClient::new()?)))
    }
}

/// Provider alias registry.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ProviderAliasRegistry {
    aliases: Vec<ProviderAlias>,
}

impl ProviderAliasRegistry {
    /// Create an empty registry.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            aliases: Vec::new(),
        }
    }

    /// Add or replace an alias.
    pub fn insert(&mut self, alias: ProviderAlias) {
        self.aliases.retain(|item| item.alias != alias.alias);
        self.aliases.push(alias);
    }

    /// Resolve an alias by name.
    #[must_use]
    pub fn resolve(&self, alias: &str) -> Option<&ProviderAlias> {
        self.aliases.iter().find(|item| item.alias == alias)
    }

    /// Build a client for an alias with an injected HTTP client.
    #[must_use]
    pub fn build_with_client(
        &self,
        alias: &str,
        client: DynHttpClient,
    ) -> Option<ProtocolModelClient> {
        self.resolve(alias)
            .map(|entry| entry.build_with_client(client))
    }
}
