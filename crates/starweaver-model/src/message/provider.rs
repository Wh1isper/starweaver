//! Provider metadata carried by canonical messages and response parts.

use serde::{Deserialize, Serialize};
use serde_json::Map;

use super::Metadata;

/// Provider-private replay metadata attached to response parts.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderPartInfo {
    /// Provider output item identifier, when the provider exposes one separately from call IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Provider that produced the part.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    /// Provider-private fields needed for same-provider replay and diagnostics.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub details: Metadata,
}

impl ProviderPartInfo {
    /// Build provider metadata for one provider output item.
    #[must_use]
    pub fn new(provider_name: impl Into<String>) -> Self {
        Self {
            id: None,
            provider_name: Some(provider_name.into()),
            details: Metadata::default(),
        }
    }

    /// Attach a provider item identifier.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        if !id.is_empty() {
            self.id = Some(id);
        }
        self
    }

    /// Attach provider details.
    #[must_use]
    pub fn with_details(mut self, details: Metadata) -> Self {
        self.details = details;
        self
    }

    /// Returns true when this metadata belongs to `provider_name`.
    #[must_use]
    pub fn is_provider(&self, provider_name: &str) -> bool {
        self.provider_name.as_deref() == Some(provider_name)
    }
}

/// Provider metadata attached to canonical responses.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProviderInfo {
    /// Provider name.
    pub name: String,
    /// Provider response identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Provider-private response metadata such as conversation IDs, request IDs, and service tiers.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub details: Metadata,
}

/// Provider-neutral finish reason.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Natural stop.
    Stop,
    /// Length or token limit.
    Length,
    /// Tool calls requested.
    ToolCalls,
    /// Content filtered by provider.
    ContentFilter,
    /// Unknown provider reason.
    Unknown,
}

impl ProviderPartInfo {
    pub(super) fn is_empty(&self) -> bool {
        self.id.is_none() && self.provider_name.is_none() && self.details.is_empty()
    }
}
