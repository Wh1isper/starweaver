use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Runtime policy for tools hidden by context-aware availability predicates.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAvailabilityPolicy {
    /// Skip unavailable tools and publish diagnostics.
    #[default]
    SkipAndReport,
    /// Fail the run before the model request when any configured tool is unavailable.
    FailRun,
}

/// Tool-level configuration stored on [`crate::AgentContext`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolConfig {
    /// Runtime behavior when a registered tool is unavailable for the current context.
    #[serde(default)]
    pub unavailable_tool_policy: ToolAvailabilityPolicy,
    /// Skip SSRF URL verification for URL-fetching tools.
    #[serde(default = "default_true")]
    pub skip_url_verification: bool,
    /// Enable document URL parsing in load-style document tools.
    #[serde(default)]
    pub enable_load_document: bool,
    /// Model used for image understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_understanding_model: Option<String>,
    /// Model used for video understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_understanding_model: Option<String>,
    /// Model used for audio understanding fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_understanding_model: Option<String>,
    /// Google Custom Search API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_search_api_key: Option<String>,
    /// Google Custom Search Engine id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub google_search_cx: Option<String>,
    /// Tavily API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tavily_api_key: Option<String>,
    /// Brave Search API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brave_search_api_key: Option<String>,
    /// Pixabay API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pixabay_api_key: Option<String>,
    /// `RapidAPI` key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rapidapi_api_key: Option<String>,
    /// Firecrawl API key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firecrawl_api_key: Option<String>,
    /// Maximum text file size the view tool will inspect.
    #[serde(default = "default_view_max_text_file_size")]
    pub view_max_text_file_size: u64,
    /// Static relaxed text view path patterns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub view_relaxed_text_patterns: Vec<String>,
    /// Runtime relaxed text view patterns keyed by source id.
    #[serde(default, skip)]
    pub view_relaxed_text_dynamic_patterns: BTreeMap<String, Vec<String>>,
    /// Maximum text file size for relaxed text paths.
    #[serde(default = "default_view_relaxed_text_file_size")]
    pub view_relaxed_text_file_size: u64,
    /// Default line limit for relaxed text paths.
    #[serde(default = "default_view_relaxed_line_limit")]
    pub view_relaxed_line_limit: usize,
    /// Default max line length for relaxed text paths.
    #[serde(default = "default_view_relaxed_max_line_length")]
    pub view_relaxed_max_line_length: usize,
    /// Maximum returned text characters for normal view output.
    #[serde(default = "default_view_max_content_chars")]
    pub view_max_content_chars: usize,
    /// Maximum returned text characters for relaxed view output.
    #[serde(default = "default_view_relaxed_max_content_chars")]
    pub view_relaxed_max_content_chars: usize,
    /// Maximum file size `edit` and `multi_edit` will process.
    #[serde(default = "default_edit_max_file_size")]
    pub edit_max_file_size: u64,
    /// Maximum file size grep will read per file.
    #[serde(default = "default_grep_max_file_size")]
    pub grep_max_file_size: u64,
    /// Maximum image size view will inline.
    #[serde(default = "default_view_max_inline_image_bytes")]
    pub view_max_inline_image_bytes: u64,
    /// Maximum video size view will inline.
    #[serde(default = "default_view_max_inline_video_bytes")]
    pub view_max_inline_video_bytes: u64,
    /// Maximum audio size view will inline.
    #[serde(default = "default_view_max_inline_audio_bytes")]
    pub view_max_inline_audio_bytes: u64,
    /// Chunk size for streamed HTTP reads.
    #[serde(default = "default_fetch_stream_chunk_size")]
    pub fetch_stream_chunk_size: usize,
    /// Maximum binary response size fetch will inline.
    #[serde(default = "default_fetch_max_inline_binary_bytes")]
    pub fetch_max_inline_binary_bytes: u64,
    /// Maximum concurrent downloads.
    #[serde(default = "default_download_max_concurrency")]
    pub download_max_concurrency: usize,
    /// Maximum file size for document conversion tools.
    #[serde(default = "default_document_max_file_size")]
    pub document_max_file_size: u64,
    /// Filesystem generic output truncation limit.
    #[serde(default = "default_filesystem_output_truncate_limit")]
    pub filesystem_output_truncate_limit: usize,
    /// Grep soft truncation threshold.
    #[serde(default = "default_grep_truncation_threshold")]
    pub grep_truncation_threshold: usize,
    /// Grep matching line max characters after truncation.
    #[serde(default = "default_grep_truncated_line_max")]
    pub grep_truncated_line_max: usize,
    /// Shell output truncation limit.
    #[serde(default = "default_shell_output_truncate_limit")]
    pub shell_output_truncate_limit: usize,
    /// Cold-start history tool return character limit.
    #[serde(default = "default_cold_start_tool_return_limit")]
    pub cold_start_tool_return_limit: usize,
}

impl Default for ToolConfig {
    fn default() -> Self {
        Self {
            unavailable_tool_policy: ToolAvailabilityPolicy::SkipAndReport,
            skip_url_verification: true,
            enable_load_document: false,
            image_understanding_model: None,
            video_understanding_model: None,
            audio_understanding_model: None,
            google_search_api_key: None,
            google_search_cx: None,
            tavily_api_key: None,
            brave_search_api_key: None,
            pixabay_api_key: None,
            rapidapi_api_key: None,
            firecrawl_api_key: None,
            view_max_text_file_size: default_view_max_text_file_size(),
            view_relaxed_text_patterns: Vec::new(),
            view_relaxed_text_dynamic_patterns: BTreeMap::new(),
            view_relaxed_text_file_size: default_view_relaxed_text_file_size(),
            view_relaxed_line_limit: default_view_relaxed_line_limit(),
            view_relaxed_max_line_length: default_view_relaxed_max_line_length(),
            view_max_content_chars: default_view_max_content_chars(),
            view_relaxed_max_content_chars: default_view_relaxed_max_content_chars(),
            edit_max_file_size: default_edit_max_file_size(),
            grep_max_file_size: default_grep_max_file_size(),
            view_max_inline_image_bytes: default_view_max_inline_image_bytes(),
            view_max_inline_video_bytes: default_view_max_inline_video_bytes(),
            view_max_inline_audio_bytes: default_view_max_inline_audio_bytes(),
            fetch_stream_chunk_size: default_fetch_stream_chunk_size(),
            fetch_max_inline_binary_bytes: default_fetch_max_inline_binary_bytes(),
            download_max_concurrency: default_download_max_concurrency(),
            document_max_file_size: default_document_max_file_size(),
            filesystem_output_truncate_limit: default_filesystem_output_truncate_limit(),
            grep_truncation_threshold: default_grep_truncation_threshold(),
            grep_truncated_line_max: default_grep_truncated_line_max(),
            shell_output_truncate_limit: default_shell_output_truncate_limit(),
            cold_start_tool_return_limit: default_cold_start_tool_return_limit(),
        }
    }
}

impl ToolConfig {
    /// Return whether the persisted config only contains default values.
    ///
    /// Runtime-only dynamic relaxed patterns do not make the config persistent.
    #[must_use]
    pub fn is_default(&self) -> bool {
        let mut persisted = self.clone();
        persisted.view_relaxed_text_dynamic_patterns.clear();
        persisted == Self::default()
    }

    /// Return all static and runtime relaxed text patterns in deterministic order.
    #[must_use]
    pub fn effective_view_relaxed_text_patterns(&self) -> Vec<&str> {
        let mut patterns = self
            .view_relaxed_text_patterns
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>();
        for source_patterns in self.view_relaxed_text_dynamic_patterns.values() {
            patterns.extend(source_patterns.iter().map(String::as_str));
        }
        patterns
    }

    /// Register runtime relaxed text patterns for a source id.
    pub fn register_view_relaxed_text_patterns(
        &mut self,
        source: impl Into<String>,
        patterns: impl IntoIterator<Item = String>,
    ) {
        let patterns = patterns
            .into_iter()
            .map(|pattern| pattern.trim().to_string())
            .filter(|pattern| !pattern.is_empty())
            .collect::<Vec<_>>();
        let source = source.into();
        if patterns.is_empty() {
            self.view_relaxed_text_dynamic_patterns.remove(&source);
        } else {
            self.view_relaxed_text_dynamic_patterns
                .insert(source, patterns);
        }
    }

    /// Remove runtime relaxed text patterns for a source id.
    pub fn unregister_view_relaxed_text_patterns(&mut self, source: &str) {
        self.view_relaxed_text_dynamic_patterns.remove(source);
    }

    /// Normalize concurrency fields after deserialization or direct mutation.
    pub fn normalize(&mut self) {
        self.download_max_concurrency = self.download_max_concurrency.max(1);
    }
}

const fn default_true() -> bool {
    true
}

const fn default_view_max_text_file_size() -> u64 {
    10 * 1024 * 1024
}

const fn default_view_relaxed_text_file_size() -> u64 {
    50 * 1024 * 1024
}

const fn default_view_relaxed_line_limit() -> usize {
    5000
}

const fn default_view_relaxed_max_line_length() -> usize {
    20_000
}

const fn default_view_max_content_chars() -> usize {
    60_000
}

const fn default_view_relaxed_max_content_chars() -> usize {
    250_000
}

const fn default_edit_max_file_size() -> u64 {
    20 * 1024 * 1024
}

const fn default_grep_max_file_size() -> u64 {
    10 * 1024 * 1024
}

const fn default_view_max_inline_image_bytes() -> u64 {
    20 * 1024 * 1024
}

const fn default_view_max_inline_video_bytes() -> u64 {
    50 * 1024 * 1024
}

const fn default_view_max_inline_audio_bytes() -> u64 {
    50 * 1024 * 1024
}

const fn default_fetch_stream_chunk_size() -> usize {
    64 * 1024
}

const fn default_fetch_max_inline_binary_bytes() -> u64 {
    30 * 1024 * 1024
}

const fn default_download_max_concurrency() -> usize {
    4
}

const fn default_document_max_file_size() -> u64 {
    200 * 1024 * 1024
}

const fn default_filesystem_output_truncate_limit() -> usize {
    20_000
}

const fn default_grep_truncation_threshold() -> usize {
    30_000
}

const fn default_grep_truncated_line_max() -> usize {
    300
}

const fn default_shell_output_truncate_limit() -> usize {
    20_000
}

const fn default_cold_start_tool_return_limit() -> usize {
    500
}
