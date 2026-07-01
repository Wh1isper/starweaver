//! Fileops-loaded skill package support.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentEvent};
use starweaver_core::Metadata;
use starweaver_environment::{DynEnvironmentProvider, EnvironmentError, FileGlobOptions};
use starweaver_model::ModelMessage;
use starweaver_runtime::{AgentCapability, AgentRunState, CapabilityResult, CapabilitySpec};
use starweaver_tools::{DynToolset, StaticToolset, ToolInstruction};
use thiserror::Error;

/// Runtime event kind emitted when scanned skills are registered for a run.
pub const SKILL_SCAN_EVENT_KIND: &str = "skills_scanned";
/// Runtime event kind emitted when a full skill body is activated.
pub const SKILL_ACTIVATION_EVENT_KIND: &str = "skill_activated";
/// Runtime event kind emitted when a skill registry reload reports changes.
pub const SKILL_RELOAD_EVENT_KIND: &str = "skills_reloaded";

/// One skill package loaded from provider-visible files.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillPackage {
    /// Stable skill name.
    pub name: String,
    /// Short model-facing description.
    pub description: String,
    /// Provider path to the `SKILL.md` file.
    pub path: String,
    /// Markdown body loaded from the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// Extra frontmatter fields preserved for hosts.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl SkillPackage {
    /// Return the compact instruction summary for this skill.
    #[must_use]
    pub fn summary_line(&self) -> String {
        format!("- {}: {} ({})", self.name, self.description, self.path)
    }
}

/// Source tier used to resolve duplicate skill names deterministically.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceKind {
    /// First-party skills bundled by a host application.
    BuiltIn,
    /// Shared user skills, usually under `.agents/skills`.
    UserShared,
    /// Starweaver-specific user skills, usually under `skills`.
    UserTool,
    /// Shared workspace skills, usually under `.agents/skills`.
    WorkspaceShared,
    /// Starweaver-specific workspace skills, usually under `skills`.
    WorkspaceTool,
    /// Caller-defined scope. Relative ordering is the order supplied by the caller.
    #[default]
    Custom,
}

impl SkillSourceKind {
    const fn precedence(self) -> u8 {
        match self {
            Self::BuiltIn => 0,
            Self::UserShared => 10,
            Self::UserTool => 20,
            Self::WorkspaceShared => 30,
            Self::WorkspaceTool => 40,
            Self::Custom => 50,
        }
    }
}

/// Skill discovery configuration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillSourceScope {
    /// Root path scanned through the environment provider.
    pub root: String,
    /// Source tier used when duplicate skill names are found.
    #[serde(default, skip_serializing_if = "is_custom_source")]
    pub source: SkillSourceKind,
    /// Directory names searched under the root.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub directories: Vec<String>,
}

impl SkillSourceScope {
    /// Build a scope with default Starweaver skill directories.
    #[must_use]
    pub fn new(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::Custom,
            directories: vec![".agents/skills".to_string(), "skills".to_string()],
        }
    }

    /// Build a first-party built-in skill scope.
    #[must_use]
    pub fn built_in(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::BuiltIn,
            directories: vec!["skills".to_string()],
        }
    }

    /// Build a shared user skill scope.
    #[must_use]
    pub fn user_shared(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::UserShared,
            directories: vec![".agents/skills".to_string()],
        }
    }

    /// Build a Starweaver-specific user skill scope.
    #[must_use]
    pub fn user_tool(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::UserTool,
            directories: vec!["skills".to_string()],
        }
    }

    /// Build a shared workspace skill scope.
    #[must_use]
    pub fn workspace_shared(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::WorkspaceShared,
            directories: vec![".agents/skills".to_string()],
        }
    }

    /// Build a Starweaver-specific workspace skill scope.
    #[must_use]
    pub fn workspace_tool(root: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            source: SkillSourceKind::WorkspaceTool,
            directories: vec!["skills".to_string()],
        }
    }

    /// Override the source tier.
    #[must_use]
    pub const fn with_source(mut self, source: SkillSourceKind) -> Self {
        self.source = source;
        self
    }

    /// Override directory names searched under this scope's root.
    #[must_use]
    pub fn with_directories(
        mut self,
        directories: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.directories = directories.into_iter().map(Into::into).collect();
        self
    }
}

/// Non-fatal skill scan diagnostic kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScanDiagnosticKind {
    /// A higher-precedence skill replaced a lower-precedence skill with the same name.
    DuplicateOverridden,
    /// A discovered skill markdown file could not be parsed.
    InvalidSkill,
    /// A discovered skill markdown file could not be read.
    UnreadableSkill,
}

/// Non-fatal skill scan diagnostic.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillScanDiagnostic {
    /// Diagnostic kind.
    pub kind: SkillScanDiagnosticKind,
    /// Source tier being scanned.
    pub source: SkillSourceKind,
    /// Skill name when it is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Path that produced the diagnostic.
    pub path: String,
    /// Replaced lower-precedence path for duplicate diagnostics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// Human-readable diagnostic detail.
    pub message: String,
}

/// Result of a lenient skill scan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillScanReport {
    /// Successfully scanned registry.
    pub registry: SkillRegistry,
    /// Non-fatal diagnostics collected while scanning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<SkillScanDiagnostic>,
}

impl SkillScanReport {
    /// Return the scanned registry.
    #[must_use]
    pub const fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Consume the report and return the scanned registry.
    #[must_use]
    pub fn into_registry(self) -> SkillRegistry {
        self.registry
    }

    /// Return non-fatal scan diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> &[SkillScanDiagnostic] {
        &self.diagnostics
    }
}

/// Skill registry reload change kind.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReloadChangeKind {
    /// A skill exists in the new registry but not the previous registry.
    Added,
    /// A skill existed in the previous registry but not the new registry.
    Removed,
    /// A skill exists in both registries but its summary metadata changed.
    Modified,
}

/// One skill registry reload change.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadChange {
    /// Change kind.
    pub kind: SkillReloadChangeKind,
    /// Skill name.
    pub name: String,
    /// Previous provider path, when one existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_path: Option<String>,
    /// Current provider path, when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// Result of reloading a skill registry from provider-visible files.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadReport {
    /// Reloaded registry.
    pub registry: SkillRegistry,
    /// Non-fatal diagnostics collected while scanning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<SkillScanDiagnostic>,
    /// Added, removed, or modified skills compared with the previous registry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<SkillReloadChange>,
}

impl SkillReloadReport {
    /// Return the reloaded registry.
    #[must_use]
    pub const fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Consume the report and return the reloaded registry.
    #[must_use]
    pub fn into_registry(self) -> SkillRegistry {
        self.registry
    }

    /// Return non-fatal scan diagnostics.
    #[must_use]
    pub fn diagnostics(&self) -> &[SkillScanDiagnostic] {
        &self.diagnostics
    }

    /// Return reload changes.
    #[must_use]
    pub fn changes(&self) -> &[SkillReloadChange] {
        &self.changes
    }
}

/// Reason a host-owned skill reload scheduler selected a reload.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillReloadReason {
    /// The configured interval elapsed.
    IntervalElapsed,
    /// Host skill inventory version changed and debounce elapsed.
    InventoryChanged,
}

/// Deterministic reload scheduling policy for host-owned skill directories.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadSchedule {
    /// Minimum interval between reloads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval_ms: Option<u64>,
    /// Debounce duration after inventory change observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_ms: Option<u64>,
}

impl SkillReloadSchedule {
    /// Create an empty schedule. It reloads only after observed inventory changes.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            interval_ms: None,
            debounce_ms: None,
        }
    }

    /// Reload after this interval elapses since the previous reload.
    #[must_use]
    pub const fn every(mut self, interval_ms: u64) -> Self {
        self.interval_ms = Some(interval_ms);
        self
    }

    /// Wait this long after an observed inventory change before reloading.
    #[must_use]
    pub const fn debounce(mut self, debounce_ms: u64) -> Self {
        self.debounce_ms = Some(debounce_ms);
        self
    }

    /// Evaluate whether a host should reload skills at `now_ms`.
    #[must_use]
    pub fn evaluate(self, state: &SkillReloadScheduleState, now_ms: u64) -> SkillReloadDecision {
        if let Some(pending_since_ms) = state.pending_since_ms {
            let debounce_ms = self.debounce_ms.unwrap_or_default();
            let elapsed_ms = now_ms.saturating_sub(pending_since_ms);
            if elapsed_ms >= debounce_ms {
                return SkillReloadDecision {
                    due: true,
                    reason: Some(SkillReloadReason::InventoryChanged),
                    next_check_after_ms: None,
                };
            }
            return SkillReloadDecision {
                due: false,
                reason: None,
                next_check_after_ms: Some(debounce_ms - elapsed_ms),
            };
        }

        if let Some(interval_ms) = self.interval_ms {
            let elapsed_ms = state
                .last_reload_ms
                .map_or(interval_ms, |last| now_ms.saturating_sub(last));
            if elapsed_ms >= interval_ms {
                return SkillReloadDecision {
                    due: true,
                    reason: Some(SkillReloadReason::IntervalElapsed),
                    next_check_after_ms: None,
                };
            }
            return SkillReloadDecision {
                due: false,
                reason: None,
                next_check_after_ms: Some(interval_ms - elapsed_ms),
            };
        }

        SkillReloadDecision::default()
    }
}

/// Host-owned mutable state used with [`SkillReloadSchedule`].
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadScheduleState {
    /// Last successful reload timestamp in host-chosen monotonic milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reload_ms: Option<u64>,
    /// Last inventory version that was successfully reloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_inventory_version: Option<String>,
    /// Pending inventory version observed by the host but not yet reloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_inventory_version: Option<String>,
    /// Timestamp when the pending inventory version was first observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_since_ms: Option<u64>,
}

impl SkillReloadScheduleState {
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

    /// Mark the currently selected or supplied inventory version as reloaded.
    pub fn mark_reloaded(&mut self, inventory_version: Option<String>, now_ms: u64) {
        let version = inventory_version
            .or_else(|| self.pending_inventory_version.clone())
            .or_else(|| self.last_inventory_version.clone());
        self.last_reload_ms = Some(now_ms);
        self.last_inventory_version = version;
        self.pending_inventory_version = None;
        self.pending_since_ms = None;
    }
}

/// Skill reload scheduler decision returned to host code.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadDecision {
    /// Whether the host should reload now.
    pub due: bool,
    /// Reason reload is due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<SkillReloadReason>,
    /// Milliseconds until the next useful schedule check when reload is not due.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_check_after_ms: Option<u64>,
}

/// Host-owned binding between skill inventory watcher signals and registry reloads.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillReloadBinding {
    /// Reload scheduling policy.
    pub schedule: SkillReloadSchedule,
    /// Mutable schedule state owned by the host.
    pub state: SkillReloadScheduleState,
}

impl SkillReloadBinding {
    /// Create a binding with empty schedule state.
    #[must_use]
    pub fn new(schedule: SkillReloadSchedule) -> Self {
        Self {
            schedule,
            state: SkillReloadScheduleState::default(),
        }
    }

    /// Create a binding from an existing state snapshot.
    #[must_use]
    pub const fn with_state(
        schedule: SkillReloadSchedule,
        state: SkillReloadScheduleState,
    ) -> Self {
        Self { schedule, state }
    }

    /// Observe a host inventory version from a watcher, cache index, or remote catalog.
    pub fn observe_inventory_version(&mut self, version: impl Into<String>, now_ms: u64) {
        self.state.observe_inventory_version(version, now_ms);
    }

    /// Evaluate whether a reload is due at `now_ms`.
    #[must_use]
    pub fn evaluate(&self, now_ms: u64) -> SkillReloadDecision {
        self.schedule.evaluate(&self.state, now_ms)
    }

    fn pending_inventory_version(&self) -> Option<String> {
        self.state.pending_inventory_version.clone()
    }

    fn mark_reloaded(&mut self, inventory_version: Option<String>, now_ms: u64) {
        self.state.mark_reloaded(inventory_version, now_ms);
    }
}

/// Result of evaluating a reload binding and optionally reloading the registry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillScheduledReloadResult {
    /// Scheduler decision evaluated for the supplied timestamp.
    pub decision: SkillReloadDecision,
    /// Inventory version selected for the reload, when one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory_version: Option<String>,
    /// Reload report when a reload was due and executed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reload: Option<SkillReloadReport>,
}

impl SkillScheduledReloadResult {
    /// Return whether the binding performed a reload.
    #[must_use]
    pub const fn reloaded(&self) -> bool {
        self.reload.is_some()
    }
}

#[derive(Clone, Debug)]
struct SkillReloadEventMetadata {
    reason: SkillReloadReason,
    inventory_version: Option<String>,
    reload_ms: u64,
}

/// Fileops-loaded skill registry.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SkillRegistry {
    packages: BTreeMap<String, SkillPackage>,
}

impl SkillRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register discovered skill markdown files as relaxed view paths on an agent context.
    pub fn register_relaxed_view_patterns(&self, context: &mut AgentContext) {
        let patterns = self.relaxed_markdown_patterns();
        if patterns.is_empty() {
            context
                .tool_config
                .unregister_view_relaxed_text_patterns(SKILL_RELAXED_VIEW_SOURCE);
        } else {
            context
                .tool_config
                .register_view_relaxed_text_patterns(SKILL_RELAXED_VIEW_SOURCE, patterns);
        }
    }

    /// Return relaxed view regex patterns for all markdown files inside skill directories.
    #[must_use]
    pub fn relaxed_markdown_patterns(&self) -> Vec<String> {
        self.packages
            .values()
            .filter_map(|package| parent_path(&normalize_skill_path(&package.path)))
            .map(|directory| format!("re:^{}/.*\\.md$", regex_escape(&directory)))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Insert or replace a skill package.
    pub fn insert(&mut self, package: SkillPackage) {
        self.packages.insert(package.name.clone(), package);
    }

    /// Return a skill by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&SkillPackage> {
        self.packages.get(name)
    }

    /// Return all skill packages in stable name order.
    #[must_use]
    pub fn packages(&self) -> Vec<SkillPackage> {
        self.packages.values().cloned().collect()
    }

    /// Return whether the registry has no skills.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Load skill summaries from provider-visible `SKILL.md` files.
    ///
    /// # Errors
    ///
    /// Returns an error when a discovered skill file is malformed.
    pub async fn scan(
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
    ) -> Result<Self, SkillError> {
        scan_skills(provider, scopes, true)
            .await
            .map(SkillScanReport::into_registry)
    }

    /// Leniently load skill summaries and return non-fatal diagnostics.
    ///
    /// Malformed or unreadable skill files are reported in the returned diagnostics
    /// while scanning continues. Directory-level provider errors remain fatal.
    ///
    /// # Errors
    ///
    /// Returns an error when a scope cannot be scanned.
    pub async fn scan_with_report(
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
    ) -> Result<SkillScanReport, SkillError> {
        scan_skills(provider, scopes, false).await
    }

    /// Load one skill body from its provider path.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed.
    pub async fn activate(
        provider: DynEnvironmentProvider,
        path: &str,
    ) -> Result<SkillPackage, SkillError> {
        let text = provider.read_text(path).await?;
        parse_skill_markdown(path, &text)
    }

    /// Load one skill body and publish an activation event on the supplied context.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or parsed.
    pub async fn activate_with_context(
        provider: DynEnvironmentProvider,
        path: &str,
        context: &mut AgentContext,
    ) -> Result<SkillPackage, SkillError> {
        let package = Self::activate(provider, path).await?;
        publish_skill_activation_event(context, &package);
        Ok(package)
    }

    /// Reload this registry from provider-visible skill files and report changes.
    ///
    /// This is an explicit host-triggered reload primitive. File watching, debounce,
    /// and scheduling belong to the application or runtime host.
    ///
    /// # Errors
    ///
    /// Returns an error when a scope cannot be scanned.
    pub async fn reload_with_report(
        &self,
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
    ) -> Result<SkillReloadReport, SkillError> {
        let report = Self::scan_with_report(provider, scopes).await?;
        Ok(SkillReloadReport {
            changes: self.reload_changes(&report.registry),
            registry: report.registry,
            diagnostics: report.diagnostics,
        })
    }

    /// Reload this registry and publish a `skills_reloaded` context event.
    ///
    /// # Errors
    ///
    /// Returns an error when a scope cannot be scanned.
    pub async fn reload_with_context(
        &self,
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
        context: &mut AgentContext,
    ) -> Result<SkillReloadReport, SkillError> {
        let report = self.reload_with_report(provider, scopes).await?;
        publish_skill_reload_event(context, &report, None);
        Ok(report)
    }

    /// Evaluate a host reload binding and reload this registry when it is due.
    ///
    /// # Errors
    ///
    /// Returns an error when a due reload cannot scan a scope.
    pub async fn reload_with_context_if_due(
        &self,
        provider: DynEnvironmentProvider,
        scopes: &[SkillSourceScope],
        context: &mut AgentContext,
        binding: &mut SkillReloadBinding,
        now_ms: u64,
    ) -> Result<SkillScheduledReloadResult, SkillError> {
        let decision = binding.evaluate(now_ms);
        let inventory_version = binding.pending_inventory_version();
        if !decision.due {
            return Ok(SkillScheduledReloadResult {
                decision,
                inventory_version,
                reload: None,
            });
        }
        let report = self.reload_with_report(provider, scopes).await?;
        publish_skill_reload_event(
            context,
            &report,
            scheduled_skill_reload_metadata(&decision, inventory_version.clone(), now_ms).as_ref(),
        );
        binding.mark_reloaded(inventory_version.clone(), now_ms);
        Ok(SkillScheduledReloadResult {
            decision,
            inventory_version,
            reload: Some(report),
        })
    }

    /// Convert loaded summaries into a model-facing instruction toolset.
    #[must_use]
    pub fn toolset(&self) -> DynToolset {
        skill_tools(self.packages())
    }

    fn reload_changes(&self, next: &Self) -> Vec<SkillReloadChange> {
        let mut changes = Vec::new();
        for (name, package) in &next.packages {
            match self.packages.get(name) {
                None => changes.push(SkillReloadChange {
                    kind: SkillReloadChangeKind::Added,
                    name: name.clone(),
                    previous_path: None,
                    path: Some(package.path.clone()),
                }),
                Some(previous) if skill_summary_changed(previous, package) => {
                    changes.push(SkillReloadChange {
                        kind: SkillReloadChangeKind::Modified,
                        name: name.clone(),
                        previous_path: Some(previous.path.clone()),
                        path: Some(package.path.clone()),
                    });
                }
                Some(_) => {}
            }
        }
        for (name, package) in &self.packages {
            if !next.packages.contains_key(name) {
                changes.push(SkillReloadChange {
                    kind: SkillReloadChangeKind::Removed,
                    name: name.clone(),
                    previous_path: Some(package.path.clone()),
                    path: None,
                });
            }
        }
        changes.sort_by(|left, right| left.name.cmp(&right.name).then(left.kind.cmp(&right.kind)));
        changes
    }
}

async fn scan_skills(
    provider: DynEnvironmentProvider,
    scopes: &[SkillSourceScope],
    strict: bool,
) -> Result<SkillScanReport, SkillError> {
    let mut report = SkillScanReport::default();
    let mut ordered_scopes = scopes.iter().enumerate().collect::<Vec<_>>();
    ordered_scopes.sort_by_key(|(index, scope)| (scope.source.precedence(), *index));
    for (_index, scope) in ordered_scopes {
        for directory in &scope.directories {
            let base = join_path(&scope.root, directory);
            let matches = match provider
                .glob(
                    &base,
                    "*/SKILL.md",
                    FileGlobOptions {
                        include_hidden: true,
                        include_ignored: true,
                        max_results: 0,
                    },
                )
                .await
            {
                Ok(matches) => matches,
                Err(EnvironmentError::NotFound(_) | EnvironmentError::AccessDenied(_)) => {
                    Vec::new()
                }
                Err(error) => return Err(SkillError::Environment(error)),
            };
            for entry in matches {
                let text = match provider.read_text(&entry.path).await {
                    Ok(text) => text,
                    Err(error) if strict => return Err(SkillError::Environment(error)),
                    Err(error) => {
                        report.diagnostics.push(SkillScanDiagnostic {
                            kind: SkillScanDiagnosticKind::UnreadableSkill,
                            source: scope.source,
                            name: None,
                            path: entry.path,
                            previous_path: None,
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                let mut package = match parse_skill_markdown(&entry.path, &text) {
                    Ok(package) => package,
                    Err(error) if strict => return Err(error),
                    Err(error) => {
                        report.diagnostics.push(SkillScanDiagnostic {
                            kind: SkillScanDiagnosticKind::InvalidSkill,
                            source: scope.source,
                            name: None,
                            path: entry.path,
                            previous_path: None,
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                package.body = None;
                let previous = report
                    .registry
                    .packages
                    .insert(package.name.clone(), package.clone());
                if let Some(previous) = previous {
                    report.diagnostics.push(SkillScanDiagnostic {
                        kind: SkillScanDiagnosticKind::DuplicateOverridden,
                        source: scope.source,
                        name: Some(package.name),
                        path: package.path,
                        previous_path: Some(previous.path),
                        message:
                            "higher-precedence skill replaced an earlier skill with the same name"
                                .to_string(),
                    });
                }
            }
        }
    }
    Ok(report)
}

/// Capability that registers discovered skill files with the runtime context.
#[derive(Clone, Debug)]
pub struct SkillDiscoveryCapability {
    registry: SkillRegistry,
    scan_diagnostics: Vec<SkillScanDiagnostic>,
}

impl SkillDiscoveryCapability {
    /// Create a capability from a scanned skill registry.
    #[must_use]
    pub const fn new(registry: SkillRegistry) -> Self {
        Self {
            registry,
            scan_diagnostics: Vec::new(),
        }
    }

    /// Create a capability from a lenient scan report, preserving diagnostics for events.
    #[must_use]
    pub fn from_report(report: SkillScanReport) -> Self {
        Self {
            registry: report.registry,
            scan_diagnostics: report.diagnostics,
        }
    }

    /// Attach scan diagnostics to the capability.
    #[must_use]
    pub fn with_scan_diagnostics(
        mut self,
        diagnostics: impl IntoIterator<Item = SkillScanDiagnostic>,
    ) -> Self {
        self.scan_diagnostics = diagnostics.into_iter().collect();
        self
    }

    /// Return the skill registry used by this capability.
    #[must_use]
    pub const fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// Return diagnostics that will be published at run start.
    #[must_use]
    pub fn scan_diagnostics(&self) -> &[SkillScanDiagnostic] {
        &self.scan_diagnostics
    }
}

#[async_trait]
impl AgentCapability for SkillDiscoveryCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("starweaver.skills.discovery")
            .with_description("Registers discovered skill markdown files as relaxed context paths.")
    }

    async fn on_run_start_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        publish_skill_scan_event(context, &self.registry, &self.scan_diagnostics);
        for package in self
            .registry
            .packages
            .values()
            .filter(|package| package.body.is_some())
        {
            publish_skill_activation_event(context, package);
        }
        Ok(())
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<ModelMessage>,
    ) -> CapabilityResult<Vec<ModelMessage>> {
        self.registry.register_relaxed_view_patterns(context);
        Ok(messages)
    }
}

/// Create a skill discovery capability from a scanned registry.
#[must_use]
pub fn skill_discovery(registry: SkillRegistry) -> Arc<dyn AgentCapability> {
    Arc::new(SkillDiscoveryCapability::new(registry))
}

/// Create a skill discovery capability from a scan report, preserving diagnostics.
#[must_use]
pub fn skill_discovery_from_report(report: SkillScanReport) -> Arc<dyn AgentCapability> {
    Arc::new(SkillDiscoveryCapability::from_report(report))
}

/// Create a toolset contributing available-skill instructions.
#[must_use]
pub fn skill_tools(packages: impl IntoIterator<Item = SkillPackage>) -> DynToolset {
    let mut lines = packages
        .into_iter()
        .map(|package| package.summary_line())
        .collect::<Vec<_>>();
    lines.sort();
    let content = if lines.is_empty() {
        "No fileops-loaded skills are currently available.".to_string()
    } else {
        format!(
            "<skill-routing-policy>\n\
Skill use is mandatory when applicable.\n\
At the start of every new user task, compare the request against <available-skills>.\n\
If one or more skills match, you MUST read the matching skill's SKILL.md before planning or executing the task.\n\
Prefer reading a possibly relevant skill over guessing from memory or improvising.\n\
After reading a skill, follow its workflow unless it conflicts with higher-priority instructions or the user's explicit request.\n\
If multiple skills match, chain them deliberately: read the most specific skill first, then read supporting skills before their workflow steps are needed.\n\
For multi-phase tasks, re-check <available-skills> at phase boundaries and activate additional skills when the next phase matches them.\n\
If you intentionally skip an apparently relevant skill, briefly state why.\n\
</skill-routing-policy>\n\n\
<available-skills>\n\
Available fileops-loaded skills:\n\
{}\n\
</available-skills>\n\n\
<skill-activation-procedure>\n\
1. Identify matching skills from the descriptions in <available-skills>.\n\
2. Read each matching skill by opening the SKILL.md file at the shown path.\n\
3. If the task has multiple phases or adjacent domains, identify and read additional skills for those phases before executing them.\n\
4. Read any additional reference files, scripts, or examples named by the activated skills.\n\
5. Use available file, shell, web, or other tools to execute the activated skills' workflows.\n\
6. Treat <available-skills> as an index only; the authoritative instructions live in SKILL.md.\n\
</skill-activation-procedure>",
            lines.join("\n")
        )
    };
    Arc::new(
        StaticToolset::new("skills")
            .with_id("skills")
            .with_instruction(ToolInstruction::new("skills", content).with_dynamic(true)),
    )
}

/// Skill loading error.
#[derive(Debug, Error)]
pub enum SkillError {
    /// File did not include frontmatter delimiters.
    #[error("invalid skill markdown: expected frontmatter delimited by ---")]
    MissingFrontmatter,
    /// Frontmatter could not be parsed.
    #[error("invalid skill frontmatter: {0}")]
    InvalidFrontmatter(String),
    /// Required field was absent.
    #[error("missing required skill field: {0}")]
    MissingField(&'static str),
    /// Provider operation failed.
    #[error(transparent)]
    Environment(#[from] starweaver_environment::EnvironmentError),
}

#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(flatten)]
    extra: Metadata,
}

/// Parse one `SKILL.md` package.
///
/// # Errors
///
/// Returns an error when frontmatter is malformed or required fields are missing.
pub fn parse_skill_markdown(path: &str, content: &str) -> Result<SkillPackage, SkillError> {
    let trimmed = content.trim();
    let body_start = trimmed
        .strip_prefix("---")
        .ok_or(SkillError::MissingFrontmatter)?
        .trim_start_matches(['\r', '\n']);
    let (frontmatter, body) = body_start
        .split_once("\n---")
        .ok_or(SkillError::MissingFrontmatter)?;
    let body = body
        .strip_prefix("---")
        .unwrap_or(body)
        .trim_start_matches(['\r', '\n'])
        .trim()
        .to_string();
    let frontmatter: SkillFrontmatter = yaml_serde::from_str(frontmatter)
        .map_err(|error| SkillError::InvalidFrontmatter(error.to_string()))?;
    Ok(SkillPackage {
        name: frontmatter.name.ok_or(SkillError::MissingField("name"))?,
        description: frontmatter
            .description
            .ok_or(SkillError::MissingField("description"))?,
        path: path.to_string(),
        body: Some(body),
        metadata: frontmatter.extra,
    })
}

const SKILL_RELAXED_VIEW_SOURCE: &str = "skills:markdown";

fn normalize_skill_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");
    if let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    normalized.trim_end_matches('/').to_string()
}

fn parent_path(path: &str) -> Option<String> {
    path.rsplit_once('/').map(|(parent, _)| parent.to_string())
}

fn regex_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if matches!(
            ch,
            '.' | '+' | '*' | '?' | '^' | '$' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

fn publish_skill_scan_event(
    context: &mut AgentContext,
    registry: &SkillRegistry,
    diagnostics: &[SkillScanDiagnostic],
) {
    context.publish_event(AgentEvent::new(
        SKILL_SCAN_EVENT_KIND,
        serde_json::json!({
            "package_count": registry.packages.len(),
            "packages": registry.packages.values().map(skill_event_summary).collect::<Vec<_>>(),
            "diagnostics": diagnostics,
        }),
    ));
}

fn publish_skill_activation_event(context: &mut AgentContext, package: &SkillPackage) {
    context.publish_event(AgentEvent::new(
        SKILL_ACTIVATION_EVENT_KIND,
        serde_json::json!({
            "name": package.name,
            "description": package.description,
            "path": package.path,
            "body_bytes": package.body.as_ref().map_or(0, String::len),
            "metadata": package.metadata,
        }),
    ));
}

fn scheduled_skill_reload_metadata(
    decision: &SkillReloadDecision,
    inventory_version: Option<String>,
    reload_ms: u64,
) -> Option<SkillReloadEventMetadata> {
    decision
        .reason
        .clone()
        .map(|reason| SkillReloadEventMetadata {
            reason,
            inventory_version,
            reload_ms,
        })
}

fn publish_skill_reload_event(
    context: &mut AgentContext,
    report: &SkillReloadReport,
    event_metadata: Option<&SkillReloadEventMetadata>,
) {
    let mut payload = serde_json::json!({
        "package_count": report.registry.packages.len(),
        "packages": report.registry.packages.values().map(skill_event_summary).collect::<Vec<_>>(),
        "diagnostics": &report.diagnostics,
        "changes": &report.changes,
    });
    if let (Some(object), Some(event_metadata)) = (payload.as_object_mut(), event_metadata) {
        object.insert("reload_scheduled".to_string(), serde_json::json!(true));
        object.insert(
            "reload_reason".to_string(),
            serde_json::to_value(&event_metadata.reason)
                .unwrap_or_else(|_| serde_json::json!("unknown")),
        );
        object.insert(
            "reload_ms".to_string(),
            serde_json::json!(event_metadata.reload_ms),
        );
        if let Some(inventory_version) = event_metadata.inventory_version.as_ref() {
            object.insert(
                "inventory_version".to_string(),
                serde_json::json!(inventory_version),
            );
        }
    }
    context.publish_event(AgentEvent::new(SKILL_RELOAD_EVENT_KIND, payload));
}

fn skill_event_summary(package: &SkillPackage) -> serde_json::Value {
    serde_json::json!({
        "name": package.name,
        "description": package.description,
        "path": package.path,
        "activated": package.body.is_some(),
    })
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_custom_source(source: &SkillSourceKind) -> bool {
    matches!(source, SkillSourceKind::Custom)
}

fn skill_summary_changed(previous: &SkillPackage, next: &SkillPackage) -> bool {
    previous.name != next.name
        || previous.description != next.description
        || previous.path != next.path
        || previous.metadata != next.metadata
        || previous.body.is_some() != next.body.is_some()
}

fn join_path(root: &str, path: &str) -> String {
    let root = if root == "/" {
        "/"
    } else {
        root.trim_end_matches('/')
    };
    let path = path.trim_matches('/');
    match (root.is_empty(), path.is_empty()) {
        (true, true) => String::new(),
        (true, false) => path.to_string(),
        (false, true) => root.to_string(),
        (false, false) if root == "/" => format!("/{path}"),
        (false, false) => format!("{root}/{path}"),
    }
}
