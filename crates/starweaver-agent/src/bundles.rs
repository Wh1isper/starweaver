//! First-party SDK tool bundles.

mod environment;
mod external;
mod helpers;
mod media;
mod task;
mod tool_proxy;
mod web;

use std::sync::Arc;

use starweaver_tools::{DynToolset, PrefixedToolset};

pub use environment::{attach_environment, filesystem_tools, shell_tools, EnvironmentHandle};
pub use external::host_operation_tools;
pub use media::{
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse,
};
pub use task::task_tools;
pub use tool_proxy::{tool_proxy_toolset, ToolProxyToolset};
pub use web::{
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem,
};

/// Create the currently implemented first-party core toolsets.
#[must_use]
pub fn core_toolsets() -> Vec<DynToolset> {
    vec![
        filesystem_tools(),
        shell_tools(),
        task_tools(),
        host_operation_tools(),
    ]
}

/// Wrap a toolset with a stable namespace prefix.
#[must_use]
pub fn namespaced_toolset(prefix: impl Into<String>, toolset: DynToolset) -> DynToolset {
    Arc::new(PrefixedToolset::new(prefix, toolset))
}
