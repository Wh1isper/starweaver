//! JSON-compatible run attachment helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Metadata;

/// JSON-compatible attachment map shared by run context, tool context, and host bindings.
///
/// Attachments are explicitly Starweaver-local execution metadata. They may be persisted in
/// session/run evidence when stored in context state, but they must not be forwarded to model
/// provider HTTP headers or provider-specific request fields by generic SDK code.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunAttachments {
    /// Attachment values keyed by stable application-defined names.
    #[serde(default, flatten)]
    pub values: Metadata,
}

impl RunAttachments {
    /// Create an empty attachment map.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create attachments from a metadata map.
    #[must_use]
    pub const fn from_metadata(values: Metadata) -> Self {
        Self { values }
    }

    /// Convert attachments into the underlying metadata map.
    #[must_use]
    pub fn into_metadata(self) -> Metadata {
        self.values
    }

    /// Return whether the attachment map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Return the number of attachments.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Get one attachment value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Insert or replace one attachment value.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        self.values.insert(key.into(), value)
    }

    /// Merge another attachment map into this one. Incoming keys replace existing values.
    pub fn merge(&mut self, other: impl Into<Metadata>) {
        self.values.extend(other.into());
    }
}

impl From<Metadata> for RunAttachments {
    fn from(values: Metadata) -> Self {
        Self::from_metadata(values)
    }
}

impl From<RunAttachments> for Metadata {
    fn from(attachments: RunAttachments) -> Self {
        attachments.into_metadata()
    }
}

impl AsRef<Metadata> for RunAttachments {
    fn as_ref(&self) -> &Metadata {
        &self.values
    }
}

impl AsMut<Metadata> for RunAttachments {
    fn as_mut(&mut self) -> &mut Metadata {
        &mut self.values
    }
}
