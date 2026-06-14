//! Capability identifiers, specs, and retry boundary metadata.

use serde::{Deserialize, Serialize};
use starweaver_core::Metadata;

/// Runtime retry boundary observed by capability hooks.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryEventKind {
    /// Output validation or output function validation requested another model turn.
    Output,
    /// A tool requested semantic retry through structured metadata.
    Tool,
}

/// Stable capability identifier.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CapabilityId(String);

impl CapabilityId {
    /// Build an identifier from a string.
    #[must_use]
    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CapabilityId {
    fn from(value: &str) -> Self {
        Self::from_string(value)
    }
}

impl From<String> for CapabilityId {
    fn from(value: String) -> Self {
        Self::from_string(value)
    }
}

/// Capability ordering constraints.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityOrdering {
    /// Capability ids that must run before this capability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<CapabilityId>,
    /// Capability ids that must run after this capability.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<CapabilityId>,
}

impl CapabilityOrdering {
    /// Require this capability to run after another capability.
    #[must_use]
    pub fn after(mut self, id: impl Into<CapabilityId>) -> Self {
        self.after.push(id.into());
        self
    }

    /// Require this capability to run before another capability.
    #[must_use]
    pub fn before(mut self, id: impl Into<CapabilityId>) -> Self {
        self.before.push(id.into());
        self
    }
}

/// Stable capability specification used for ordering and reconstruction evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilitySpec {
    /// Stable capability id.
    pub id: CapabilityId,
    /// Human-readable description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Ordering constraints.
    #[serde(default)]
    pub ordering: CapabilityOrdering,
    /// Whether the capability can be loaded on demand by a host registry.
    #[serde(default)]
    pub on_demand: bool,
    /// Additional metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl CapabilitySpec {
    /// Build a capability spec from an id.
    #[must_use]
    pub fn new(id: impl Into<CapabilityId>) -> Self {
        Self {
            id: id.into(),
            description: None,
            ordering: CapabilityOrdering::default(),
            on_demand: false,
            metadata: Metadata::default(),
        }
    }

    /// Attach a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Attach ordering constraints.
    #[must_use]
    pub fn with_ordering(mut self, ordering: CapabilityOrdering) -> Self {
        self.ordering = ordering;
        self
    }

    /// Mark the capability as on-demand loadable.
    #[must_use]
    pub const fn with_on_demand(mut self, on_demand: bool) -> Self {
        self.on_demand = on_demand;
        self
    }

    /// Attach metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}
