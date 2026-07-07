//! Fixed two-tool proxy over many underlying toolsets.

mod format;
mod index;
mod inner;

use std::{collections::BTreeSet, sync::Arc};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};
use starweaver_core::Metadata;
use starweaver_model::ToolDefinition;
use thiserror::Error;

use crate::{
    DynTool, DynToolset, Tool, ToolContext, ToolError, ToolInstruction, ToolResult,
    ToolUserInputPreprocessResult, Toolset, typed_json_tool,
};

use inner::ToolProxyInner;

const TOOL_SEARCH_NAME: &str = "tool_search";
const TOOL_SEARCH_TOOLSET_NAME: &str = "tool_search";
const TOOL_SEARCH_INSTRUCTION_GROUP: &str = "tool-search";
const SEARCH_TOOLS_NAME: &str = "search_tools";
const CALL_TOOL_NAME: &str = "call_tool";
const PREFIXED_SEARCH_TOOL_SUFFIX: &str = "search_tool";
const PREFIXED_CALL_TOOL_SUFFIX: &str = "call_tool";
const TOOL_PROXY_NAME: &str = "tool_proxy";
const TOOL_PROXY_INSTRUCTION_GROUP: &str = "tool-proxy";

/// Event emitted when a dynamic tool-search query is invalid.
pub const TOOL_SEARCH_FAILED_EVENT_KIND: &str = "tool_search_failed";
/// Event emitted when a dynamic tool-search query returns no matches.
pub const TOOL_SEARCH_NO_MATCH_EVENT_KIND: &str = "tool_search_no_match";
/// Event emitted when host code clears loaded dynamic tool-search state.
pub const TOOL_SEARCH_INVALIDATED_EVENT_KIND: &str = "tool_search_invalidated";
/// Event emitted when host code refreshes dynamic tool-search inventory.
pub const TOOL_SEARCH_REFRESHED_EVENT_KIND: &str = "tool_search_refreshed";
const TOOL_SEARCH_INITIALIZED_EVENT_KIND: &str = "tool_search_initialized";
const TOOL_SEARCH_LOADED_EVENT_KIND: &str = "tool_search_loaded";

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(super) struct SearchToolsArgs {
    /// Natural language or keyword query to search for tools.
    pub(super) query: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub(super) struct CallToolArgs {
    /// Name of the tool to invoke.
    pub(super) name: String,
    /// Arguments to pass to the tool, matching its parameter schema.
    #[serde(default)]
    pub(super) arguments: Value,
}

/// Error returned when a proxy tool name prefix is invalid.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
#[error(
    "ToolProxyToolset name prefix must start with a letter and contain only letters, numbers, and underscores"
)]
pub struct ToolProxyNamePrefixError {
    prefix: String,
}

impl ToolProxyNamePrefixError {
    const fn new(prefix: String) -> Self {
        Self { prefix }
    }

    /// Return the rejected prefix text.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
}

fn normalize_prefix(prefix: String) -> Result<Option<String>, ToolProxyNamePrefixError> {
    let normalized = prefix.trim().trim_matches('_').to_string();
    if normalized.is_empty() {
        return Ok(None);
    }

    let mut chars = normalized.chars();
    let Some(first) = chars.next() else {
        return Ok(None);
    };
    if !first.is_ascii_alphabetic() || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return Err(ToolProxyNamePrefixError::new(prefix));
    }
    Ok(Some(normalized))
}

/// Create a fixed two-tool proxy over many underlying toolsets.
#[must_use]
pub fn dynamic_tool_proxy(toolsets: Vec<DynToolset>) -> DynToolset {
    Arc::new(ToolProxyToolset::new(toolsets))
}

/// Create a direct dynamic tool-search toolset.
///
/// The returned toolset initially exposes only `tool_search`. Wrapped tools are
/// registered in the tool registry but hidden by context-aware availability until
/// `tool_search` records them in `AgentContext.tool_search_loaded_tools` or
/// `AgentContext.tool_search_loaded_namespaces`.
#[must_use]
pub fn dynamic_tool_search(toolsets: Vec<DynToolset>) -> DynToolset {
    Arc::new(ToolSearchToolset::new(toolsets))
}

/// Result of a direct tool-search load operation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchLoadResult {
    /// Tool names loaded into the agent context.
    pub loaded_tools: Vec<String>,
    /// Namespace ids loaded into the agent context.
    pub loaded_namespaces: Vec<String>,
}

impl ToolSearchLoadResult {
    /// Return whether no tools or namespaces were loaded.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.loaded_tools.is_empty() && self.loaded_namespaces.is_empty()
    }
}

/// Result of host-driven tool-search loaded-state invalidation.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchInvalidationResult {
    /// Host-supplied invalidation reason.
    pub reason: String,
    /// Tool names removed from loaded state.
    pub removed_loaded_tools: Vec<String>,
    /// Namespace ids removed from loaded state.
    pub removed_loaded_namespaces: Vec<String>,
    /// Loaded tools that remain after invalidation.
    pub retained_loaded_tools: Vec<String>,
    /// Loaded namespaces that remain after invalidation.
    pub retained_loaded_namespaces: Vec<String>,
}

impl ToolSearchInvalidationResult {
    /// Return whether invalidation removed no loaded entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.removed_loaded_tools.is_empty() && self.removed_loaded_namespaces.is_empty()
    }
}

/// Result of host-driven tool-search inventory refresh.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchRefreshResult {
    /// Fresh inventory and availability report.
    pub report: ToolSearchInitializationReport,
    /// Tool names removed from loaded state because they are stale or unavailable.
    pub removed_loaded_tools: Vec<String>,
    /// Namespace ids removed from loaded state because they are stale or unavailable.
    pub removed_loaded_namespaces: Vec<String>,
    /// Loaded tools still valid after refresh.
    pub retained_loaded_tools: Vec<String>,
    /// Loaded namespaces still valid after refresh.
    pub retained_loaded_namespaces: Vec<String>,
}

impl ToolSearchRefreshResult {
    /// Return whether refresh removed no loaded entries.
    #[must_use]
    pub const fn removed_nothing(&self) -> bool {
        self.removed_loaded_tools.is_empty() && self.removed_loaded_namespaces.is_empty()
    }
}

/// Reason a host-owned tool-search refresh scheduler selected a refresh.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSearchRefreshReason {
    /// The configured interval elapsed.
    IntervalElapsed,
    /// Host inventory version changed and debounce elapsed.
    InventoryChanged,
}

/// Deterministic refresh scheduling policy for host-owned dynamic tool libraries.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchRefreshSchedule {
    /// Minimum interval between refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// Debounce duration after inventory change observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
}

impl ToolSearchRefreshSchedule {
    /// Create an empty schedule. It refreshes only after observed inventory changes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            interval_ms: None,
            debounce_ms: None,
        }
    }

    /// Refresh after this interval elapses since the previous refresh.
    #[must_use]
    pub const fn every(mut self, interval_ms: u64) -> Self {
        self.interval_ms = Some(interval_ms);
        self
    }

    /// Wait this long after an observed inventory change before refreshing.
    #[must_use]
    pub const fn debounce(mut self, debounce_ms: u64) -> Self {
        self.debounce_ms = Some(debounce_ms);
        self
    }

    /// Evaluate whether a host should call `refresh_loaded_state` at `now_ms`.
    #[must_use]
    pub fn evaluate(
        self,
        state: &ToolSearchRefreshScheduleState,
        now_ms: u64,
    ) -> ToolSearchRefreshDecision {
        if let Some(pending_since_ms) = state.pending_since_ms {
            let debounce_ms = self.debounce_ms.unwrap_or_default();
            let elapsed_ms = now_ms.saturating_sub(pending_since_ms);
            if elapsed_ms >= debounce_ms {
                return ToolSearchRefreshDecision {
                    due: true,
                    reason: Some(ToolSearchRefreshReason::InventoryChanged),
                    next_check_after_ms: None,
                };
            }
            return ToolSearchRefreshDecision {
                due: false,
                reason: None,
                next_check_after_ms: Some(debounce_ms - elapsed_ms),
            };
        }

        if let Some(interval_ms) = self.interval_ms {
            let elapsed_ms = state
                .last_refresh_ms
                .map_or(interval_ms, |last| now_ms.saturating_sub(last));
            if elapsed_ms >= interval_ms {
                return ToolSearchRefreshDecision {
                    due: true,
                    reason: Some(ToolSearchRefreshReason::IntervalElapsed),
                    next_check_after_ms: None,
                };
            }
            return ToolSearchRefreshDecision {
                due: false,
                reason: None,
                next_check_after_ms: Some(interval_ms - elapsed_ms),
            };
        }

        ToolSearchRefreshDecision::default()
    }
}

/// Host-owned mutable state used with [`ToolSearchRefreshSchedule`].
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchRefreshScheduleState {
    /// Last successful refresh timestamp in host-chosen monotonic milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh_ms: Option<u64>,
    /// Last inventory version that was successfully refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_inventory_version: Option<String>,
    /// Pending inventory version observed by the host but not yet refreshed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_inventory_version: Option<String>,
    /// Timestamp when the pending inventory version was first observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_since_ms: Option<u64>,
}

impl ToolSearchRefreshScheduleState {
    /// Observe a host inventory version. A new version starts the debounce window.
    pub fn observe_inventory_version(&mut self, version: impl Into<String>, now_ms: u64) {
        let version = version.into();
        if self.last_inventory_version.as_deref() == Some(version.as_str())
            || self.pending_inventory_version.as_deref() == Some(version.as_str())
        {
            return;
        }
        self.pending_inventory_version = Some(version);
        self.pending_since_ms = Some(now_ms);
    }

    /// Mark the currently selected or supplied inventory version as refreshed.
    pub fn mark_refreshed(&mut self, inventory_version: Option<String>, now_ms: u64) {
        let version = inventory_version
            .or_else(|| self.pending_inventory_version.clone())
            .or_else(|| self.last_inventory_version.clone());
        self.last_refresh_ms = Some(now_ms);
        self.last_inventory_version = version;
        self.pending_inventory_version = None;
        self.pending_since_ms = None;
    }
}

/// Refresh scheduler decision returned to host code.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchRefreshDecision {
    /// Whether the host should refresh now.
    pub due: bool,
    /// Reason refresh is due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<ToolSearchRefreshReason>,
    /// Milliseconds until the next useful schedule check when refresh is not due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_check_after_ms: Option<u64>,
}

/// Host-owned binding between inventory watcher signals and dynamic tool-search refreshes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchRefreshBinding {
    /// Refresh scheduling policy.
    pub schedule: ToolSearchRefreshSchedule,
    /// Mutable schedule state owned by the host.
    pub state: ToolSearchRefreshScheduleState,
}

impl ToolSearchRefreshBinding {
    /// Create a binding with empty schedule state.
    #[must_use]
    pub fn new(schedule: ToolSearchRefreshSchedule) -> Self {
        Self {
            schedule,
            state: ToolSearchRefreshScheduleState::default(),
        }
    }

    /// Create a binding from an existing state snapshot.
    #[must_use]
    pub const fn with_state(
        schedule: ToolSearchRefreshSchedule,
        state: ToolSearchRefreshScheduleState,
    ) -> Self {
        Self { schedule, state }
    }

    /// Observe a host inventory version from a watcher, cache index, or remote catalog.
    pub fn observe_inventory_version(&mut self, version: impl Into<String>, now_ms: u64) {
        self.state.observe_inventory_version(version, now_ms);
    }

    /// Evaluate whether a refresh is due at `now_ms`.
    #[must_use]
    pub fn evaluate(&self, now_ms: u64) -> ToolSearchRefreshDecision {
        self.schedule.evaluate(&self.state, now_ms)
    }

    fn pending_inventory_version(&self) -> Option<String> {
        self.state.pending_inventory_version.clone()
    }

    fn mark_refreshed(&mut self, inventory_version: Option<String>, now_ms: u64) {
        self.state.mark_refreshed(inventory_version, now_ms);
    }
}

/// Result of evaluating a refresh binding and optionally refreshing loaded state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchScheduledRefreshResult {
    /// Scheduler decision evaluated for the supplied timestamp.
    pub decision: ToolSearchRefreshDecision,
    /// Inventory version selected for the refresh, when one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_version: Option<String>,
    /// Refresh result when a refresh was due and executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh: Option<ToolSearchRefreshResult>,
}

impl ToolSearchScheduledRefreshResult {
    /// Return whether the binding performed a refresh.
    #[must_use]
    pub const fn refreshed(&self) -> bool {
        self.refresh.is_some()
    }
}

#[derive(Clone, Debug)]
struct ToolSearchRefreshEventMetadata {
    reason: ToolSearchRefreshReason,
    inventory_version: Option<String>,
    refresh_ms: u64,
}

/// Tool-search namespace initialization status.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSearchNamespaceStatus {
    /// Namespace has at least one currently available tool.
    Connected,
    /// Namespace is known but contains no tools.
    Empty,
    /// Namespace has tools, but all are unavailable in the supplied context.
    Unavailable,
}

/// Initialization details for one dynamic tool-search namespace.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchNamespaceReport {
    /// Namespace id.
    pub namespace: String,
    /// Initialization status.
    pub status: ToolSearchNamespaceStatus,
    /// Names of all indexed tools in this namespace.
    pub tools: Vec<String>,
    /// Number of indexed tools in this namespace.
    pub total_tools: usize,
    /// Number of currently available tools when a context was supplied.
    pub available_tools: usize,
    /// Number of currently unavailable tools when a context was supplied.
    pub unavailable_tools: usize,
}

/// Initialization report for a dynamic tool-search surface.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolSearchInitializationReport {
    /// Toolset surface name.
    pub toolset_name: String,
    /// Visible search tool name.
    pub search_tool_name: String,
    /// Total indexed direct tools.
    pub total_tools: usize,
    /// Number of namespace groups.
    pub total_namespaces: usize,
    /// Loose tools outside namespace groups.
    pub loose_tools: Vec<String>,
    /// Namespace initialization reports.
    pub namespaces: Vec<ToolSearchNamespaceReport>,
    /// Number of currently available tools when a context was supplied.
    pub available_tools: usize,
    /// Number of currently unavailable tools when a context was supplied.
    pub unavailable_tools: usize,
    /// Whether availability predicates were evaluated against a context.
    pub availability_checked: bool,
    /// Maximum number of search matches considered for each query.
    pub max_results: usize,
}

/// Fixed two-tool proxy for dynamic tool discovery and invocation.
///
/// The proxy exposes `search_tools` and `call_tool` by default while keeping all
/// wrapped tool definitions out of the model-visible tool list until the model
/// searches for them. Use [`ToolProxyToolset::try_with_name_prefix`] to expose
/// `{prefix}_search_tool` and `{prefix}_call_tool` instead when multiple proxy
/// surfaces need stable, non-conflicting names.
#[derive(Clone)]
pub struct ToolProxyToolset {
    inner: Arc<ToolProxyInner>,
}

impl ToolProxyToolset {
    /// Build a proxy over wrapped toolsets.
    #[must_use]
    pub fn new(toolsets: Vec<DynToolset>) -> Self {
        Self {
            inner: Arc::new(ToolProxyInner::new(toolsets)),
        }
    }

    /// Set a stable prefix for the visible proxy tool names.
    ///
    /// A prefix is trimmed and surrounding underscores are removed. Empty prefixes
    /// restore the default unprefixed names. Non-empty prefixes must start with an
    /// ASCII letter and contain only ASCII letters, ASCII digits, and underscores.
    ///
    /// # Errors
    ///
    /// Returns [`ToolProxyNamePrefixError`] when the normalized prefix is not a valid
    /// model-facing tool-name prefix.
    pub fn try_with_name_prefix(
        mut self,
        prefix: impl Into<String>,
    ) -> Result<Self, ToolProxyNamePrefixError> {
        let prefix = normalize_prefix(prefix.into())?;
        Arc::make_mut(&mut self.inner).set_prefix(prefix);
        Ok(self)
    }

    /// Set a stable prefix for the visible proxy tool names.
    ///
    /// Prefer [`Self::try_with_name_prefix`] when the prefix comes from user input.
    ///
    /// # Panics
    ///
    /// Panics when the prefix is not a valid model-facing tool-name prefix.
    #[must_use]
    pub fn with_name_prefix(self, prefix: impl Into<String>) -> Self {
        match self.try_with_name_prefix(prefix) {
            Ok(proxy) => proxy,
            Err(error) => panic!("{error}"),
        }
    }

    /// Return the optional visible proxy tool prefix.
    #[must_use]
    pub fn prefix(&self) -> Option<&str> {
        self.inner.prefix()
    }

    /// Return the visible search proxy tool name.
    #[must_use]
    pub fn search_tool_name(&self) -> &str {
        self.inner.search_tool_name()
    }

    /// Return the visible call proxy tool name.
    #[must_use]
    pub fn call_tool_name(&self) -> &str {
        self.inner.call_tool_name()
    }

    /// Set namespace descriptions by toolset id.
    #[must_use]
    pub fn with_namespace_descriptions(
        mut self,
        descriptions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let descriptions = descriptions
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        Arc::make_mut(&mut self.inner).set_namespace_descriptions(descriptions);
        self
    }

    /// Set the maximum number of search matches.
    #[must_use]
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        Arc::make_mut(&mut self.inner).set_max_results(max_results);
        self
    }

    /// Return the wrapped toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[DynToolset] {
        self.inner.toolsets()
    }

    /// Build a current initialization report for the wrapped tool library.
    #[must_use]
    pub fn initialization_report(
        &self,
        context: Option<&AgentContext>,
    ) -> ToolSearchInitializationReport {
        self.inner
            .initialization_report(self.inner.name(), self.inner.search_tool_name(), context)
    }

    /// Publish a current initialization report into an agent context.
    pub fn publish_initialization_report(
        &self,
        context: &mut AgentContext,
    ) -> ToolSearchInitializationReport {
        let report = self.initialization_report(Some(context));
        publish_tool_search_report(context, TOOL_SEARCH_INITIALIZED_EVENT_KIND, &report);
        report
    }

    /// Rebuild the stateless search index and return a current report.
    #[must_use]
    pub fn refresh_report(&self, context: Option<&AgentContext>) -> ToolSearchInitializationReport {
        self.initialization_report(context)
    }

    /// Publish a current refresh report into an agent context.
    pub fn publish_refresh_report(
        &self,
        context: &mut AgentContext,
    ) -> ToolSearchInitializationReport {
        let report = self.refresh_report(Some(context));
        publish_tool_search_report(context, TOOL_SEARCH_REFRESHED_EVENT_KIND, &report);
        report
    }

    /// Clear host-visible loaded tool-search state for this proxy surface.
    pub fn invalidate_loaded_state(
        &self,
        context: &mut AgentContext,
        reason: impl Into<String>,
    ) -> ToolSearchInvalidationResult {
        invalidate_tool_search_state(context, reason)
    }

    /// Refresh current tool inventory and prune stale or unavailable loaded state.
    pub fn refresh_loaded_state(&self, context: &mut AgentContext) -> ToolSearchRefreshResult {
        refresh_tool_search_state(
            context,
            &self.inner,
            self.inner.name(),
            self.inner.search_tool_name(),
            &[self.inner.search_tool_name(), self.inner.call_tool_name()],
        )
    }

    /// Evaluate a host refresh binding and refresh loaded state when it is due.
    pub fn refresh_loaded_state_if_due(
        &self,
        context: &mut AgentContext,
        binding: &mut ToolSearchRefreshBinding,
        now_ms: u64,
    ) -> ToolSearchScheduledRefreshResult {
        let decision = binding.evaluate(now_ms);
        let inventory_version = binding.pending_inventory_version();
        if !decision.due {
            return ToolSearchScheduledRefreshResult {
                decision,
                inventory_version,
                refresh: None,
            };
        }
        let refresh = refresh_tool_search_state_with_metadata(
            context,
            &self.inner,
            self.inner.name(),
            self.inner.search_tool_name(),
            &[self.inner.search_tool_name(), self.inner.call_tool_name()],
            scheduled_refresh_metadata(&decision, inventory_version.clone(), now_ms).as_ref(),
        );
        binding.mark_refreshed(inventory_version.clone(), now_ms);
        ToolSearchScheduledRefreshResult {
            decision,
            inventory_version,
            refresh: Some(refresh),
        }
    }
}

impl Toolset for ToolProxyToolset {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let search_inner = self.inner.clone();
        let call_inner = self.inner.clone();
        vec![
            Arc::new(typed_json_tool::<SearchToolsArgs, _, _>(
                self.inner.search_tool_name().to_string(),
                Some("Search for available tools by keyword, description, namespace, or parameter schema. Returns XML with full parameter schemas.".to_string()),
                move |context, arguments| {
                    let inner = search_inner.clone();
                    async move { Ok(inner.search_tools(&context, &arguments)) }
                },
            )),
            Arc::new(typed_json_tool::<CallToolArgs, _, _>(
                self.inner.call_tool_name().to_string(),
                Some("Invoke an available tool by name with arguments matching the tool's parameter schema.".to_string()),
                move |context, arguments| {
                    let inner = call_inner.clone();
                    async move { inner.call_tool(context, arguments).await }
                },
            )),
        ]
    }

    fn max_retries(&self) -> Option<usize> {
        Some(3)
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        vec![ToolInstruction::new(
            self.inner.instruction_group().to_string(),
            self.inner.proxy_instruction(),
        )]
    }
}

/// Direct dynamic tool discovery toolset.
///
/// Unlike [`ToolProxyToolset`], this exposes discovered tools as ordinary tools on
/// the next model turn. The search tool should be registered after always-visible
/// toolsets so loaded tools append after stable core tools.
#[derive(Clone)]
pub struct ToolSearchToolset {
    inner: Arc<ToolProxyInner>,
    search_tool_name: String,
}

impl ToolSearchToolset {
    /// Build a direct tool-search surface over wrapped toolsets.
    #[must_use]
    pub fn new(toolsets: Vec<DynToolset>) -> Self {
        Self {
            inner: Arc::new(ToolProxyInner::new(toolsets)),
            search_tool_name: TOOL_SEARCH_NAME.to_string(),
        }
    }

    /// Return the visible search tool name.
    #[must_use]
    pub fn search_tool_name(&self) -> &str {
        &self.search_tool_name
    }

    /// Set namespace descriptions by toolset id.
    #[must_use]
    pub fn with_namespace_descriptions(
        mut self,
        descriptions: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        let descriptions = descriptions
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        Arc::make_mut(&mut self.inner).set_namespace_descriptions(descriptions);
        self
    }

    /// Set the maximum number of search matches.
    #[must_use]
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        Arc::make_mut(&mut self.inner).set_max_results(max_results);
        self
    }

    /// Return the wrapped toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[DynToolset] {
        self.inner.toolsets()
    }

    /// Build a current initialization report for the wrapped tool library.
    #[must_use]
    pub fn initialization_report(
        &self,
        context: Option<&AgentContext>,
    ) -> ToolSearchInitializationReport {
        self.inner
            .initialization_report(TOOL_SEARCH_TOOLSET_NAME, &self.search_tool_name, context)
    }

    /// Publish a current initialization report into an agent context.
    pub fn publish_initialization_report(
        &self,
        context: &mut AgentContext,
    ) -> ToolSearchInitializationReport {
        let report = self.initialization_report(Some(context));
        publish_tool_search_report(context, TOOL_SEARCH_INITIALIZED_EVENT_KIND, &report);
        report
    }

    /// Rebuild the stateless search index and return a current report.
    #[must_use]
    pub fn refresh_report(&self, context: Option<&AgentContext>) -> ToolSearchInitializationReport {
        self.initialization_report(context)
    }

    /// Publish a current refresh report into an agent context.
    pub fn publish_refresh_report(
        &self,
        context: &mut AgentContext,
    ) -> ToolSearchInitializationReport {
        let report = self.refresh_report(Some(context));
        publish_tool_search_report(context, TOOL_SEARCH_REFRESHED_EVENT_KIND, &report);
        report
    }

    /// Clear loaded direct tool-search state after host cache invalidation.
    pub fn invalidate_loaded_state(
        &self,
        context: &mut AgentContext,
        reason: impl Into<String>,
    ) -> ToolSearchInvalidationResult {
        invalidate_tool_search_state(context, reason)
    }

    /// Refresh direct tool inventory and prune stale or unavailable loaded state.
    pub fn refresh_loaded_state(&self, context: &mut AgentContext) -> ToolSearchRefreshResult {
        refresh_tool_search_state(
            context,
            &self.inner,
            TOOL_SEARCH_TOOLSET_NAME,
            &self.search_tool_name,
            &[self.search_tool_name.as_str()],
        )
    }

    /// Evaluate a host refresh binding and refresh loaded state when it is due.
    pub fn refresh_loaded_state_if_due(
        &self,
        context: &mut AgentContext,
        binding: &mut ToolSearchRefreshBinding,
        now_ms: u64,
    ) -> ToolSearchScheduledRefreshResult {
        let decision = binding.evaluate(now_ms);
        let inventory_version = binding.pending_inventory_version();
        if !decision.due {
            return ToolSearchScheduledRefreshResult {
                decision,
                inventory_version,
                refresh: None,
            };
        }
        let refresh = refresh_tool_search_state_with_metadata(
            context,
            &self.inner,
            TOOL_SEARCH_TOOLSET_NAME,
            &self.search_tool_name,
            &[self.search_tool_name.as_str()],
            scheduled_refresh_metadata(&decision, inventory_version.clone(), now_ms).as_ref(),
        );
        binding.mark_refreshed(inventory_version.clone(), now_ms);
        ToolSearchScheduledRefreshResult {
            decision,
            inventory_version,
            refresh: Some(refresh),
        }
    }

    /// Preload every tool in a namespace into an agent context.
    ///
    /// Returns an empty result when the namespace is unknown.
    pub fn preload_namespace(
        &self,
        context: &mut AgentContext,
        namespace: &str,
    ) -> ToolSearchLoadResult {
        let namespace = namespace.trim();
        if namespace.is_empty() {
            return ToolSearchLoadResult::default();
        }
        let index = self
            .inner
            .index_tools_with_extra_hidden_names(&[self.search_tool_name.as_str()]);
        let loaded_tools = index
            .tools_for_namespace(namespace)
            .into_iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        if loaded_tools.is_empty() {
            return ToolSearchLoadResult::default();
        }
        let result = ToolSearchLoadResult {
            loaded_tools,
            loaded_namespaces: vec![namespace.to_string()],
        };
        record_tool_search_load_event(context, &result);
        result
    }
}

impl Toolset for ToolSearchToolset {
    fn name(&self) -> &str {
        TOOL_SEARCH_TOOLSET_NAME
    }

    fn get_tools(&self) -> Vec<DynTool> {
        let hidden_search_name = self.search_tool_name.clone();
        let mut tools = Vec::new();
        let search_inner = self.inner.clone();
        tools.push(Arc::new(typed_json_tool::<SearchToolsArgs, _, _>(
            self.search_tool_name.clone(),
            Some(
                "Search for available tools and load matching tools for the next model turn."
                    .to_string(),
            ),
            move |context, arguments| {
                let inner = search_inner.clone();
                let hidden_search_name = hidden_search_name.clone();
                async move {
                    Ok(search_direct_tools(
                        &inner,
                        &context,
                        &arguments,
                        &hidden_search_name,
                    ))
                }
            },
        )) as DynTool);

        let index = self
            .inner
            .index_tools_with_extra_hidden_names(&[self.search_tool_name.as_str()]);
        for tool in index.tools.values() {
            tools.push(Arc::new(SearchLoadedTool {
                inner: tool.clone(),
            }) as DynTool);
        }
        tools
    }

    fn max_retries(&self) -> Option<usize> {
        Some(3)
    }

    fn get_instructions(&self) -> Vec<ToolInstruction> {
        let index = self
            .inner
            .index_tools_with_extra_hidden_names(&[self.search_tool_name.as_str()]);
        let mut lines = vec![
            format!(
                "Use {} to search available tools by keyword, action, namespace, or parameter name.",
                self.search_tool_name
            ),
            "Matching tools are loaded for the next model turn as direct callable tools."
                .to_string(),
            "Namespace matches load every tool in that namespace atomically.".to_string(),
        ];
        let tool_count = index.tools.len();
        if tool_count > 0 {
            lines.push(format!(
                "There are {tool_count} tools available through direct tool search."
            ));
        }
        if !index.namespace_tools.is_empty() {
            lines.push("Available tool namespaces:".to_string());
            for (namespace, tools) in index.namespace_tools {
                lines.push(format!("- {namespace} ({} tools)", tools.len()));
            }
        }
        vec![ToolInstruction::new(
            TOOL_SEARCH_INSTRUCTION_GROUP,
            lines.join("\n"),
        )]
    }
}

#[derive(Clone)]
struct SearchLoadedTool {
    inner: index::IndexedTool,
}

#[async_trait]
impl Tool for SearchLoadedTool {
    fn name(&self) -> &str {
        &self.inner.name
    }

    fn description(&self) -> Option<&str> {
        self.inner.tool.description()
    }

    fn parameters_schema(&self) -> Value {
        self.inner.tool.parameters_schema()
    }

    fn metadata(&self) -> Metadata {
        let mut metadata = self.inner.tool.metadata();
        metadata.insert("tool_search_direct".to_string(), serde_json::json!(true));
        if let Some(namespace) = self.inner.namespace.as_ref() {
            metadata.insert(
                "tool_search_namespace".to_string(),
                serde_json::json!(namespace),
            );
        }
        metadata
    }

    fn max_retries(&self) -> Option<usize> {
        self.inner.tool.max_retries()
    }

    fn timeout_ms(&self) -> Option<u64> {
        self.inner.tool.timeout_ms()
    }

    fn return_schema(&self) -> Option<Value> {
        self.inner.tool.return_schema()
    }

    fn strict_schema(&self) -> Option<bool> {
        self.inner.tool.strict_schema()
    }

    fn sequential(&self) -> Option<bool> {
        self.inner.tool.sequential()
    }

    fn is_available(&self, context: &AgentContext) -> bool {
        self.inner.tool.is_available(context) && self.is_loaded(context)
    }

    fn prepare_definition(
        &self,
        context: &AgentContext,
        definition: ToolDefinition,
    ) -> Option<ToolDefinition> {
        self.inner.tool.prepare_definition(context, definition)
    }

    async fn call(&self, context: ToolContext, arguments: Value) -> Result<ToolResult, ToolError> {
        let Some(handle) = context.dependency::<AgentContextHandle>() else {
            return Err(ToolError::UserError {
                tool: self.inner.name.clone(),
                message: "direct tool-search calls require AgentContextHandle".to_string(),
            });
        };
        if !self.is_loaded(&handle.snapshot()) {
            return Err(ToolError::UserError {
                tool: self.inner.name.clone(),
                message: format!(
                    "tool is not loaded; call {TOOL_SEARCH_NAME} before invoking {}",
                    self.inner.name
                ),
            });
        }
        self.inner.tool.call(context, arguments).await
    }

    async fn preprocess_user_input(
        &self,
        context: ToolContext,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        self.inner
            .tool
            .preprocess_user_input(context, user_input)
            .await
    }
}

impl SearchLoadedTool {
    fn is_loaded(&self, context: &AgentContext) -> bool {
        let state = context.tool_search_state();
        state
            .loaded_tools
            .iter()
            .any(|tool| tool == &self.inner.name)
            || self.inner.namespace.as_ref().is_some_and(|namespace| {
                state
                    .loaded_namespaces
                    .iter()
                    .any(|loaded| loaded == namespace)
            })
    }
}

fn search_direct_tools(
    inner: &ToolProxyInner,
    context: &ToolContext,
    arguments: &SearchToolsArgs,
    search_tool_name: &str,
) -> ToolResult {
    publish_tool_search_initialization_event(
        inner,
        context,
        TOOL_SEARCH_TOOLSET_NAME,
        search_tool_name,
    );
    if arguments.query.trim().is_empty() {
        publish_tool_search_query_event(
            context,
            TOOL_SEARCH_FAILED_EVENT_KIND,
            search_tool_name,
            &arguments.query,
            "empty_query",
            "Parameter 'query' is required.",
        );
        return ToolResult::new(serde_json::json!({
            "error": "Parameter 'query' is required.",
            "loaded_tools": [],
            "loaded_namespaces": [],
        }));
    }

    let tools = inner.search_matching_tools(&arguments.query, &[search_tool_name]);
    let loaded_tools = tools.values().map(direct_tool_summary).collect::<Vec<_>>();
    let loaded_namespaces = tools
        .values()
        .filter_map(|tool| tool.namespace.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let message = if loaded_tools.is_empty() {
        publish_tool_search_query_event(
            context,
            TOOL_SEARCH_NO_MATCH_EVENT_KIND,
            search_tool_name,
            &arguments.query,
            "no_match",
            "No tools matched the query.",
        );
        format!(
            "No tools found for query {:?}. Try a different keyword, action, namespace, or parameter name.",
            arguments.query
        )
    } else {
        ToolProxyInner::record_loaded_tools(context, tools.values());
        format!(
            "Loaded {} tool(s) for the next model turn.",
            loaded_tools.len()
        )
    };
    ToolResult::new(serde_json::json!({
        "query": arguments.query,
        "message": message,
        "loaded_tools": loaded_tools,
        "loaded_namespaces": loaded_namespaces,
    }))
}

fn publish_tool_search_initialization_event(
    inner: &ToolProxyInner,
    context: &ToolContext,
    toolset_name: &str,
    search_tool_name: &str,
) {
    let Some(handle) = context.dependency::<AgentContextHandle>() else {
        return;
    };
    handle.update(|agent_context| {
        let report =
            inner.initialization_report(toolset_name, search_tool_name, Some(agent_context));
        publish_tool_search_report(agent_context, TOOL_SEARCH_INITIALIZED_EVENT_KIND, &report);
    });
}

pub(super) fn publish_tool_search_query_event(
    context: &ToolContext,
    kind: &'static str,
    search_tool_name: &str,
    query: &str,
    error_kind: &str,
    message: &str,
) {
    let Some(handle) = context.dependency::<AgentContextHandle>() else {
        return;
    };
    handle.update(|agent_context| {
        agent_context.publish_event(AgentEvent::new(
            kind,
            serde_json::json!({
                "search_tool_name": search_tool_name,
                "query": query,
                "error_kind": error_kind,
                "message": message,
            }),
        ));
    });
}

fn record_tool_search_load_event(context: &mut AgentContext, result: &ToolSearchLoadResult) {
    context.record_tool_search_loaded(
        result.loaded_tools.clone(),
        result.loaded_namespaces.clone(),
    );
    context.publish_event(AgentEvent::new(
        TOOL_SEARCH_LOADED_EVENT_KIND,
        serde_json::json!({
            "loaded_tools": result.loaded_tools,
            "loaded_namespaces": result.loaded_namespaces,
        }),
    ));
}

fn publish_tool_search_report(
    context: &mut AgentContext,
    kind: &'static str,
    report: &ToolSearchInitializationReport,
) {
    context.publish_event(AgentEvent::new(
        kind,
        serde_json::to_value(report).unwrap_or_else(|_| serde_json::json!({})),
    ));
}

fn invalidate_tool_search_state(
    context: &mut AgentContext,
    reason: impl Into<String>,
) -> ToolSearchInvalidationResult {
    let reason = reason.into();
    let removed = context.clear_tool_search_loaded();
    let result = ToolSearchInvalidationResult {
        reason,
        removed_loaded_tools: removed.removed_tools,
        removed_loaded_namespaces: removed.removed_namespaces,
        retained_loaded_tools: context.tool_search_loaded_tools.clone(),
        retained_loaded_namespaces: context.tool_search_loaded_namespaces.clone(),
    };
    context.publish_event(AgentEvent::new(
        TOOL_SEARCH_INVALIDATED_EVENT_KIND,
        serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({})),
    ));
    result
}

fn refresh_tool_search_state(
    context: &mut AgentContext,
    inner: &ToolProxyInner,
    toolset_name: &str,
    search_tool_name: &str,
    extra_hidden_names: &[&str],
) -> ToolSearchRefreshResult {
    refresh_tool_search_state_with_metadata(
        context,
        inner,
        toolset_name,
        search_tool_name,
        extra_hidden_names,
        None,
    )
}

fn refresh_tool_search_state_with_metadata(
    context: &mut AgentContext,
    inner: &ToolProxyInner,
    toolset_name: &str,
    search_tool_name: &str,
    extra_hidden_names: &[&str],
    event_metadata: Option<&ToolSearchRefreshEventMetadata>,
) -> ToolSearchRefreshResult {
    let index = inner.index_tools_with_extra_hidden_names(extra_hidden_names);
    let valid_tools = index
        .tools
        .iter()
        .filter(|(_, tool)| tool.tool.is_available(context))
        .map(|(name, _)| name.clone())
        .collect::<BTreeSet<_>>();
    let valid_namespaces = index
        .namespace_tools
        .iter()
        .filter(|(_, tools)| tools.iter().any(|tool| valid_tools.contains(tool)))
        .map(|(namespace, _)| namespace.clone())
        .collect::<BTreeSet<_>>();
    let removed = context.retain_tool_search_loaded(
        |tool| valid_tools.contains(tool),
        |namespace| valid_namespaces.contains(namespace),
    );
    let report = inner.initialization_report(toolset_name, search_tool_name, Some(context));
    let result = ToolSearchRefreshResult {
        report,
        removed_loaded_tools: removed.removed_tools,
        removed_loaded_namespaces: removed.removed_namespaces,
        retained_loaded_tools: context.tool_search_loaded_tools.clone(),
        retained_loaded_namespaces: context.tool_search_loaded_namespaces.clone(),
    };
    publish_tool_search_refresh_result(context, &result, event_metadata);
    result
}

fn scheduled_refresh_metadata(
    decision: &ToolSearchRefreshDecision,
    inventory_version: Option<String>,
    refresh_ms: u64,
) -> Option<ToolSearchRefreshEventMetadata> {
    decision
        .reason
        .clone()
        .map(|reason| ToolSearchRefreshEventMetadata {
            reason,
            inventory_version,
            refresh_ms,
        })
}

fn publish_tool_search_refresh_result(
    context: &mut AgentContext,
    result: &ToolSearchRefreshResult,
    event_metadata: Option<&ToolSearchRefreshEventMetadata>,
) {
    let mut payload =
        serde_json::to_value(&result.report).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = payload.as_object_mut() {
        object.insert(
            "removed_loaded_tools".to_string(),
            serde_json::json!(result.removed_loaded_tools),
        );
        object.insert(
            "removed_loaded_namespaces".to_string(),
            serde_json::json!(result.removed_loaded_namespaces),
        );
        object.insert(
            "retained_loaded_tools".to_string(),
            serde_json::json!(result.retained_loaded_tools),
        );
        object.insert(
            "retained_loaded_namespaces".to_string(),
            serde_json::json!(result.retained_loaded_namespaces),
        );
        if let Some(event_metadata) = event_metadata {
            object.insert("refresh_scheduled".to_string(), serde_json::json!(true));
            object.insert(
                "refresh_reason".to_string(),
                serde_json::to_value(&event_metadata.reason)
                    .unwrap_or_else(|_| serde_json::json!("unknown")),
            );
            object.insert(
                "refresh_ms".to_string(),
                serde_json::json!(event_metadata.refresh_ms),
            );
            if let Some(inventory_version) = event_metadata.inventory_version.as_ref() {
                object.insert(
                    "inventory_version".to_string(),
                    serde_json::json!(inventory_version),
                );
            }
        }
    }
    context.publish_event(AgentEvent::new(TOOL_SEARCH_REFRESHED_EVENT_KIND, payload));
}

fn direct_tool_summary(tool: &index::IndexedTool) -> Value {
    serde_json::json!({
        "name": tool.name,
        "description": tool.description,
        "namespace": tool.namespace,
        "toolset": tool.toolset,
        "parameters": tool.parameters,
    })
}
