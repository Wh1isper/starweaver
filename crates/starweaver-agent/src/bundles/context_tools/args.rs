use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct SummarizeArgs {
    /// Context summary to preserve across context handoff.
    pub(super) content: String,
    /// File paths the resumed agent may need to inspect after summary.
    ///
    /// Only the paths are added to a reminder; file contents are not loaded into context.
    #[serde(default)]
    pub(super) auto_load_files: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct NoteSetArgs {
    /// Unique key for the note entry.
    pub(super) key: String,
    /// Content to store. Omit or set to null to delete the entry.
    #[serde(default)]
    pub(super) value: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct NoteGetArgs {
    /// The note key to retrieve. Omit to list all notes.
    #[serde(default)]
    pub(super) key: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct ThinkingArgs {
    /// A thought in markdown format.
    #[serde(alias = "content")]
    pub(super) thought: String,
}
