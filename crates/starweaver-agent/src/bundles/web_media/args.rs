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

const fn default_search_num() -> u8 {
    10
}
