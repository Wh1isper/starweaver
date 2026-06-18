//! First-party SDK tool bundles.

mod environment;
mod external;
mod helpers;
mod runtime_context;
mod skills;
mod task;

use std::sync::Arc;

use starweaver_tools::{DynToolset, PrefixedToolset};

pub use environment::{
    attach_environment, attach_process_shell, attach_shell_review, attach_shell_review_handle,
    environment_toolsets, filesystem_tools, process_shell_toolsets, shell_tools,
    EnvironmentContextCapability, EnvironmentHandle, ProcessShellHandle, ShellReviewAction,
    ShellReviewConfig, ShellReviewContextSnapshot, ShellReviewDecision, ShellReviewHandle,
    ShellReviewPreviousDecision, ShellReviewRecord, ShellReviewRequest, ShellReviewRiskLevel,
    DEFAULT_SHELL_REVIEW_PROMPT,
};
pub use external::{
    host_operation_tools, HostMediaCapabilities, HostMediaUnderstandingClient,
    HostMediaUnderstandingClientHandle, MediaUnderstandingRequest, MediaUnderstandingResponse,
};
pub use external::{
    HostScrapeClient, HostScrapeClientHandle, HostSearchClient, HostSearchClientHandle,
    ScrapeRequest, ScrapeResponse, SearchRequest, SearchResponse, SearchResultItem,
};
pub use runtime_context::RuntimeContextCapability;
pub use skills::{
    parse_skill_markdown, skill_discovery, skill_discovery_from_report, skill_tools,
    SkillDiscoveryCapability, SkillError, SkillPackage, SkillRegistry, SkillReloadBinding,
    SkillReloadChange, SkillReloadChangeKind, SkillReloadDecision, SkillReloadReason,
    SkillReloadReport, SkillReloadSchedule, SkillReloadScheduleState, SkillScanDiagnostic,
    SkillScanDiagnosticKind, SkillScanReport, SkillScheduledReloadResult, SkillSourceKind,
    SkillSourceScope, SKILL_ACTIVATION_EVENT_KIND, SKILL_RELOAD_EVENT_KIND, SKILL_SCAN_EVENT_KIND,
};
pub use starweaver_tools::{dynamic_tool_proxy, ToolProxyNamePrefixError, ToolProxyToolset};
pub use task::task_tools;

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
