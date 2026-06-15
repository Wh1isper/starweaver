//! Serializable state domains for agent context.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// In-memory state store for context domains.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateStore {
    domains: BTreeMap<String, Value>,
}

impl StateStore {
    /// Create an empty state store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a domain value.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.domains.insert(key.into(), value);
    }

    /// Get a domain value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.domains.get(key)
    }

    /// Remove a domain value.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.domains.remove(key)
    }

    /// Return whether the store has no domains.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.domains.is_empty()
    }

    /// Return all domains.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)]
    pub fn domains(&self) -> &BTreeMap<String, Value> {
        &self.domains
    }
}
