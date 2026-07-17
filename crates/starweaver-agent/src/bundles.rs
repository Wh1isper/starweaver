//! First-party SDK tool bundles.

mod context_tools;
mod environment;
mod helpers;
mod output;
mod runtime_context;
mod session_management;
mod skills;
mod task;
mod user_input;
mod web_media;

use std::sync::Arc;

use starweaver_tools::{DynToolset, PrefixedToolset};

pub use context_tools::context_tools;
pub use environment::{
    DEFAULT_SHELL_REVIEW_PROMPT, EnvironmentContextCapability, EnvironmentHandle,
    ProcessShellHandle, ShellReviewAction, ShellReviewConfig, ShellReviewContextSnapshot,
    ShellReviewDecision, ShellReviewHandle, ShellReviewPreviousDecision, ShellReviewRecord,
    ShellReviewRequest, ShellReviewRiskLevel, attach_environment, attach_process_shell,
    attach_shell_review, attach_shell_review_handle, environment_toolsets, filesystem_tools,
    process_shell_toolsets, shell_tools,
};
pub use runtime_context::RuntimeContextCapability;
pub use session_management::{
    AgentSessionControl, AgentSessionControlHandle, AgentSessionQuery, AgentSessionQueryHandle,
    agent_session_control_tools, agent_session_query_tools, attach_agent_session_control,
    attach_agent_session_query,
};
pub use skills::{
    SKILL_ACTIVATION_EVENT_KIND, SKILL_RELOAD_EVENT_KIND, SKILL_SCAN_EVENT_KIND,
    SkillDiscoveryCapability, SkillError, SkillPackage, SkillRegistry, SkillReloadBinding,
    SkillReloadChange, SkillReloadChangeKind, SkillReloadDecision, SkillReloadReason,
    SkillReloadReport, SkillReloadSchedule, SkillReloadScheduleState, SkillScanDiagnostic,
    SkillScanDiagnosticKind, SkillScanReport, SkillScheduledReloadResult, SkillSourceKind,
    SkillSourceScope, parse_skill_markdown, skill_discovery, skill_discovery_from_report,
    skill_tools,
};
pub use starweaver_tools::{ToolProxyNamePrefixError, ToolProxyToolset, dynamic_tool_proxy};
pub use task::task_tools;
pub use user_input::{
    ASK_USER_QUESTION_TOOL_NAME, AskUserQuestionArgs, AskUserQuestionResult,
    CLARIFYING_ANSWERS_METADATA_KEY, CLARIFYING_QUESTIONS_REQUEST_KIND, ClarifyingQuestion,
    ClarifyingQuestionAnswers, ClarifyingQuestionOption, normalize_clarifying_question_answers,
    resolve_clarifying_question_answers, user_input_tools,
};
pub use web_media::{
    HostMediaCapabilities, HostMediaUnderstandingClient, HostMediaUnderstandingClientHandle,
    MediaUnderstandingRequest, MediaUnderstandingResponse, host_io_tools,
};
pub use web_media::{
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
        context_tools(),
        host_io_tools(),
    ]
}

/// Wrap a toolset with a stable namespace prefix.
#[must_use]
pub fn namespaced_toolset(prefix: impl Into<String>, toolset: DynToolset) -> DynToolset {
    Arc::new(PrefixedToolset::new(prefix, toolset))
}
