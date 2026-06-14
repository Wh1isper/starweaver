//! Serializable note store carried by context state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Serializable note store carried by context state.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct NoteStore {
    notes: BTreeMap<String, String>,
}

impl NoteStore {
    /// Create an empty note store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a note value.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.notes.insert(key.into(), value.into());
    }

    /// Get a note value.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.notes.get(key).map(String::as_str)
    }

    /// Delete a note value and return whether it existed.
    pub fn delete(&mut self, key: &str) -> bool {
        self.notes.remove(key).is_some()
    }

    /// Return all notes sorted by key.
    #[must_use]
    pub fn entries(&self) -> Vec<(String, String)> {
        self.notes
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Return a serializable copy of all notes.
    #[must_use]
    pub fn to_map(&self) -> BTreeMap<String, String> {
        self.notes.clone()
    }

    /// Restore notes from exported data.
    #[must_use]
    pub const fn from_map(notes: BTreeMap<String, String>) -> Self {
        Self { notes }
    }

    /// Return whether the store has no notes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}
