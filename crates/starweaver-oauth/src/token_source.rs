//! Store-backed OAuth token sources.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;

use crate::{
    error::{OAuthError, OAuthResult},
    store::OAuthStore,
    types::{OAuthProviderRecord, OAuthProviderRefresher, OAuthTokenSource, TokenSnapshot},
};

/// Token source backed by `OAuthStore` and a provider-specific refresher.
#[derive(Clone)]
pub struct StoreBackedTokenSource {
    provider_name: String,
    store: OAuthStore,
    refresh_provider: Arc<dyn OAuthProviderRefresher>,
}

impl StoreBackedTokenSource {
    /// Create a store-backed token source.
    pub fn new(
        provider_name: impl Into<String>,
        store: OAuthStore,
        refresh_provider: Arc<dyn OAuthProviderRefresher>,
    ) -> Self {
        Self {
            provider_name: provider_name.into(),
            store,
            refresh_provider,
        }
    }

    /// Return the provider name.
    pub fn provider_name(&self) -> &str {
        &self.provider_name
    }

    /// Return the backing store.
    pub const fn store(&self) -> &OAuthStore {
        &self.store
    }
}

#[async_trait]
impl OAuthTokenSource for StoreBackedTokenSource {
    async fn get_token(&self) -> OAuthResult<TokenSnapshot> {
        let record = self
            .store
            .get_provider(&self.provider_name)?
            .ok_or_else(|| OAuthError::NotLoggedIn {
                provider: self.provider_name.clone(),
            })?;
        Ok(snapshot_from_record(&self.provider_name, &record))
    }

    async fn refresh_token(&self) -> OAuthResult<TokenSnapshot> {
        let record = self
            .store
            .get_provider(&self.provider_name)?
            .ok_or_else(|| OAuthError::NotLoggedIn {
                provider: self.provider_name.clone(),
            })?;
        let refreshed = self.refresh_provider.refresh_provider(&record).await?;
        self.store
            .set_provider(&self.provider_name, refreshed.clone())?;
        Ok(snapshot_from_record(&self.provider_name, &refreshed))
    }
}

pub fn snapshot_from_record(provider_name: &str, record: &OAuthProviderRecord) -> TokenSnapshot {
    TokenSnapshot {
        provider_name: provider_name.to_string(),
        access_token: record.tokens.access_token.clone(),
        account: record.account.clone(),
        base_url: record.base_url.clone(),
        metadata: BTreeMap::new(),
    }
}
