use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct SearchArgs {
    /// The search query.
    pub(super) query: String,
    /// Number of results to return.
    #[serde(default = "default_search_num")]
    pub(super) num: u8,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct UrlArgs {
    /// URL of the resource.
    pub(super) url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct FetchArgs {
    /// URL of the web resource to fetch.
    pub(super) url: String,
    /// Only check existence without downloading content.
    #[serde(default)]
    pub(super) head_only: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct DownloadArgs {
    /// List of URLs to download.
    pub(super) urls: Vec<String>,
    /// Directory where files should be saved.
    pub(super) save_dir: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct SummarizeArgs {
    /// Context summary to preserve across context handoff.
    pub(super) content: String,
    /// File paths to auto-load after summary.
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

const fn default_search_num() -> u8 {
    10
}
