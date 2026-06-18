use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use starweaver_context::AgentContext;
use starweaver_core::{AgentId, RunId, TaskId};
use starweaver_environment::DynEnvironmentProvider;
use starweaver_runtime::{Agent as RuntimeAgent, AgentCapability, AgentError, CapabilityBundle};
use starweaver_usage::Usage;

use crate::presets::{AgentSpec, AgentSpecError, AgentSpecRegistry};

use super::{SubagentCapabilityInheritancePolicy, SubagentTask, SubagentToolInheritancePolicy};

/// Shared subagent execution hook.
pub type DynSubagentExecutionHook = Arc<dyn SubagentExecutionHook>;

/// Metadata passed to subagent execution hooks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentExecutionMetadata {
    /// Subagent name.
    pub name: String,
    /// Delegated task id.
    pub task_id: TaskId,
    /// Application task metadata.
    #[serde(default)]
    pub task_metadata: serde_json::Value,
    /// Parent agent id.
    pub parent_agent_id: AgentId,
    /// Parent run id when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Child agent id selected for this delegated run.
    pub child_agent_id: AgentId,
}

impl SubagentExecutionMetadata {
    pub(crate) fn new(
        name: &str,
        task: &SubagentTask,
        parent_context: &AgentContext,
        child_context: &AgentContext,
    ) -> Self {
        Self {
            name: name.to_string(),
            task_id: task.id.clone(),
            task_metadata: task.metadata.clone(),
            parent_agent_id: parent_context.agent_id.clone(),
            parent_run_id: parent_context.run_id.clone(),
            child_agent_id: child_context.agent_id.clone(),
        }
    }
}

/// Result observed by subagent execution hooks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SubagentExecutionOutcome {
    /// Subagent run completed.
    Completed {
        /// Final text output.
        output: String,
        /// Child run id when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
        /// Child usage snapshot.
        usage: Usage,
    },
    /// Subagent run failed.
    Failed {
        /// Error text.
        error: String,
        /// Child run id when available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        run_id: Option<RunId>,
    },
}

/// Hook around delegated subagent execution.
#[async_trait]
pub trait SubagentExecutionHook: Send + Sync {
    /// Called after child context construction and before the child run starts.
    ///
    /// # Errors
    ///
    /// Returning an error fails the delegated call before the child model request.
    async fn before_subagent_run(
        &self,
        _metadata: SubagentExecutionMetadata,
        _child_context: &mut AgentContext,
    ) -> Result<(), AgentError> {
        Ok(())
    }

    /// Called after the child run finishes or fails, before parent context absorption.
    ///
    /// # Errors
    ///
    /// Returning an error fails the delegated call. When the child already failed, the original
    /// child error remains the primary returned error.
    async fn after_subagent_run(
        &self,
        _metadata: SubagentExecutionMetadata,
        _child_context: &AgentContext,
        _outcome: SubagentExecutionOutcome,
    ) -> Result<(), AgentError> {
        Ok(())
    }
}

/// Registered subagent configuration for SDK-level delegation.
#[derive(Clone)]
pub struct SubagentConfig {
    /// Subagent name exposed to application delegation policies.
    pub name: String,
    /// Optional subagent description.
    pub description: Option<String>,
    /// Nested agent runtime.
    pub agent: Arc<RuntimeAgent>,
    /// Tool inheritance policy applied before every delegated run.
    pub tool_inheritance: SubagentToolInheritancePolicy,
    /// Capability inheritance policy applied before every delegated run.
    pub capability_inheritance: SubagentCapabilityInheritancePolicy,
    pub(crate) inherited_capabilities: Vec<Arc<dyn AgentCapability>>,
    pub(crate) inherited_capability_bundles: Vec<Arc<dyn CapabilityBundle>>,
    /// Optional child-owned environment provider applied to delegated child contexts.
    pub environment: Option<DynEnvironmentProvider>,
    pub(crate) execution_hooks: Vec<DynSubagentExecutionHook>,
}

impl SubagentConfig {
    /// Build a subagent configuration.
    #[must_use]
    pub fn new(name: impl Into<String>, agent: Arc<RuntimeAgent>) -> Self {
        Self {
            name: name.into(),
            description: None,
            agent,
            tool_inheritance: SubagentToolInheritancePolicy::default(),
            capability_inheritance: SubagentCapabilityInheritancePolicy::default(),
            inherited_capabilities: Vec::new(),
            inherited_capability_bundles: Vec::new(),
            environment: None,
            execution_hooks: Vec::new(),
        }
    }

    /// Build an executable subagent configuration from a declarative agent spec.
    ///
    /// # Errors
    ///
    /// Returns an error when the spec references a model, toolset, subagent, capability, or
    /// policy preset that the registry cannot resolve.
    pub fn from_agent_spec(
        spec: &AgentSpec,
        registry: &AgentSpecRegistry,
        tool_inheritance: SubagentToolInheritancePolicy,
    ) -> Result<Self, AgentSpecError> {
        let agent = Arc::new(spec.builder(registry)?.build());
        let mut config =
            Self::new(spec.name.clone(), agent).with_tool_inheritance(tool_inheritance);
        config = config
            .with_capability_inheritance(capability_inheritance_from_metadata(&spec.metadata));
        if let Some(description) = spec.description.clone() {
            config = config.with_description(description);
        }
        if let Some(environment) = spec.materialized_environment_provider(registry)? {
            config = config.with_environment(environment);
        }
        Ok(config)
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the tool inheritance policy used for this subagent.
    #[must_use]
    pub fn with_tool_inheritance(mut self, policy: SubagentToolInheritancePolicy) -> Self {
        self.tool_inheritance = policy;
        self
    }

    /// Set the capability inheritance policy used for this subagent.
    #[must_use]
    pub fn with_capability_inheritance(
        mut self,
        policy: SubagentCapabilityInheritancePolicy,
    ) -> Self {
        self.capability_inheritance = policy;
        self
    }

    pub(crate) fn with_resolved_capability_inheritance(
        mut self,
        parent_capabilities: &[Arc<dyn AgentCapability>],
        parent_capability_bundles: &[Arc<dyn CapabilityBundle>],
    ) -> Self {
        self.inherited_capabilities.clear();
        self.inherited_capability_bundles.clear();
        if self.capability_inheritance.hooks {
            self.inherited_capabilities.extend(
                parent_capabilities
                    .iter()
                    .filter(|capability| self.capability_inheritance.allows_hook(capability))
                    .cloned(),
            );
        }
        if self.capability_inheritance.capability_bundles {
            self.inherited_capability_bundles.extend(
                parent_capability_bundles
                    .iter()
                    .filter(|bundle| self.capability_inheritance.allows_bundle(bundle))
                    .cloned(),
            );
        }
        self
    }

    /// Attach a child-owned environment provider for delegated runs.
    #[must_use]
    pub fn with_environment(mut self, provider: DynEnvironmentProvider) -> Self {
        self.environment = Some(provider);
        self
    }

    /// Attach an execution hook around delegated child runs.
    #[must_use]
    pub fn with_execution_hook(mut self, hook: DynSubagentExecutionHook) -> Self {
        self.execution_hooks.push(hook);
        self
    }

    /// Return the child-owned environment provider when configured.
    #[must_use]
    pub fn environment_provider(&self) -> Option<DynEnvironmentProvider> {
        self.environment.clone()
    }
}

fn capability_inheritance_from_metadata(
    metadata: &serde_json::Map<String, serde_json::Value>,
) -> SubagentCapabilityInheritancePolicy {
    let inherit_hooks = metadata
        .get("inherit_hooks")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let inherit_capabilities = metadata
        .get("inherit_capabilities")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let denied_capabilities = metadata
        .get("denied_capabilities")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default();
    SubagentCapabilityInheritancePolicy::default()
        .with_hooks(inherit_hooks)
        .with_capability_bundles(inherit_capabilities)
        .with_denied_capabilities(denied_capabilities)
}
