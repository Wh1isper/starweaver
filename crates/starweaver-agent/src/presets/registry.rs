//! Agent spec registry resolution.

use std::{collections::BTreeMap, sync::Arc};

use starweaver_environment::DynEnvironmentProvider;
use starweaver_model::ModelAdapter;
use starweaver_runtime::{CapabilityBundle, CapabilitySpec};
use starweaver_tools::DynToolset;

use crate::{SkillRegistry, SubagentConfig};

use super::types::{
    AgentSpecError, ApprovalPolicyPreset, DurabilityPolicyPreset, EnvironmentPolicyPreset,
    HostAdapterSpec, McpServerSpec, ObservabilityPolicyPreset, RetryPolicyPreset,
    StreamingPolicyPreset, ToolsetWrapperSpec,
};

/// Host-provided materializer for custom `AgentSpec` toolset wrappers.
pub type AgentSpecToolsetWrapperFactory = Arc<
    dyn Fn(&ToolsetWrapperSpec, &AgentSpecRegistry) -> Result<DynToolset, AgentSpecError>
        + Send
        + Sync,
>;

/// Registry used to resolve spec references into runtime objects.
#[derive(Clone, Default)]
pub struct AgentSpecRegistry {
    pub(super) models: BTreeMap<String, Arc<dyn ModelAdapter>>,
    pub(super) toolsets: Vec<DynToolset>,
    pub(super) toolsets_by_key: BTreeMap<String, DynToolset>,
    pub(super) subagents: Vec<SubagentConfig>,
    pub(super) subagents_by_name: BTreeMap<String, SubagentConfig>,
    pub(super) approval_presets: BTreeMap<String, ApprovalPolicyPreset>,
    pub(super) retry_presets: BTreeMap<String, RetryPolicyPreset>,
    pub(super) streaming_presets: BTreeMap<String, StreamingPolicyPreset>,
    pub(super) observability_presets: BTreeMap<String, ObservabilityPolicyPreset>,
    pub(super) environment_presets: BTreeMap<String, EnvironmentPolicyPreset>,
    pub(super) durability_presets: BTreeMap<String, DurabilityPolicyPreset>,
    pub(super) host_adapters: BTreeMap<String, HostAdapterSpec>,
    pub(super) mcp_servers: BTreeMap<String, McpServerSpec>,
    pub(super) capabilities: BTreeMap<String, CapabilitySpec>,
    pub(super) capability_bundles: BTreeMap<String, Arc<dyn CapabilityBundle>>,
    pub(super) skill_registries: BTreeMap<String, SkillRegistry>,
    pub(super) environment_providers: BTreeMap<String, DynEnvironmentProvider>,
    pub(super) toolset_wrapper_factories: BTreeMap<String, AgentSpecToolsetWrapperFactory>,
}

impl AgentSpecRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a model id.
    #[must_use]
    pub fn with_model(mut self, id: impl Into<String>, model: Arc<dyn ModelAdapter>) -> Self {
        self.models.insert(id.into(), model);
        self
    }

    /// Register a toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: DynToolset) -> Self {
        self.register_toolset_keys(&toolset);
        self.toolsets.push(toolset);
        self
    }

    /// Register a toolset under an additional caller-provided alias.
    #[must_use]
    pub fn with_toolset_alias(mut self, alias: impl Into<String>, toolset: DynToolset) -> Self {
        let name = toolset.name().to_string();
        let id = toolset.id().map(str::to_string);
        self.toolsets_by_key.insert(name, toolset.clone());
        if let Some(id) = id {
            self.toolsets_by_key.insert(id, toolset.clone());
        }
        self.toolsets_by_key.insert(alias.into(), toolset);
        self
    }

    /// Register a host-defined toolset wrapper materializer by wrapper kind.
    #[must_use]
    pub fn with_toolset_wrapper_factory<F>(mut self, kind: impl Into<String>, factory: F) -> Self
    where
        F: Fn(&ToolsetWrapperSpec, &Self) -> Result<DynToolset, AgentSpecError>
            + Send
            + Sync
            + 'static,
    {
        self.toolset_wrapper_factories
            .insert(kind.into(), Arc::new(factory));
        self
    }

    /// Register a subagent.
    #[must_use]
    pub fn with_subagent(mut self, subagent: SubagentConfig) -> Self {
        if let Some(existing) = self
            .subagents_by_name
            .insert(subagent.name.clone(), subagent.clone())
        {
            self.subagents
                .retain(|registered| registered.name != existing.name);
        }
        self.subagents.push(subagent);
        self
    }

    /// Register an approval preset.
    #[must_use]
    pub fn with_approval_preset(
        mut self,
        name: impl Into<String>,
        preset: ApprovalPolicyPreset,
    ) -> Self {
        self.approval_presets.insert(name.into(), preset);
        self
    }

    /// Register a retry preset.
    #[must_use]
    pub fn with_retry_preset(mut self, name: impl Into<String>, preset: RetryPolicyPreset) -> Self {
        self.retry_presets.insert(name.into(), preset);
        self
    }

    /// Register a streaming preset.
    #[must_use]
    pub fn with_streaming_preset(
        mut self,
        name: impl Into<String>,
        preset: StreamingPolicyPreset,
    ) -> Self {
        self.streaming_presets.insert(name.into(), preset);
        self
    }

    /// Register an observability preset.
    #[must_use]
    pub fn with_observability_preset(
        mut self,
        name: impl Into<String>,
        preset: ObservabilityPolicyPreset,
    ) -> Self {
        self.observability_presets.insert(name.into(), preset);
        self
    }

    /// Register an environment preset.
    #[must_use]
    pub fn with_environment_preset(
        mut self,
        name: impl Into<String>,
        preset: EnvironmentPolicyPreset,
    ) -> Self {
        self.environment_presets.insert(name.into(), preset);
        self
    }

    /// Register a durability preset.
    #[must_use]
    pub fn with_durability_preset(
        mut self,
        name: impl Into<String>,
        preset: DurabilityPolicyPreset,
    ) -> Self {
        self.durability_presets.insert(name.into(), preset);
        self
    }

    /// Register a host adapter by stable name.
    #[must_use]
    pub fn with_host_adapter(mut self, name: impl Into<String>, adapter: HostAdapterSpec) -> Self {
        self.host_adapters.insert(name.into(), adapter);
        self
    }

    /// Register an MCP server by stable name.
    #[must_use]
    pub fn with_mcp_server(mut self, name: impl Into<String>, server: McpServerSpec) -> Self {
        self.mcp_servers.insert(name.into(), server);
        self
    }

    /// Register a capability spec by stable id or alias.
    #[must_use]
    pub fn with_capability(mut self, name: impl Into<String>, capability: CapabilitySpec) -> Self {
        self.capabilities.insert(name.into(), capability);
        self
    }

    /// Register an executable capability bundle by stable id or alias.
    #[must_use]
    pub fn with_capability_bundle(
        mut self,
        name: impl Into<String>,
        bundle: Arc<dyn CapabilityBundle>,
    ) -> Self {
        let name = name.into();
        self.capabilities.insert(name.clone(), bundle.spec());
        self.capability_bundles.insert(name, bundle);
        self
    }

    /// Register a host-scanned skill registry by provider-visible root or stable alias.
    #[must_use]
    pub fn with_skill_registry(mut self, root: impl Into<String>, registry: SkillRegistry) -> Self {
        self.skill_registries.insert(root.into(), registry);
        self
    }

    /// Register a host-owned environment provider by stable profile name.
    #[must_use]
    pub fn with_environment_provider(
        mut self,
        name: impl Into<String>,
        provider: DynEnvironmentProvider,
    ) -> Self {
        self.environment_providers.insert(name.into(), provider);
        self
    }

    pub(super) fn model(&self, id: &str) -> Result<Option<Arc<dyn ModelAdapter>>, AgentSpecError> {
        if let Some(model) = self.models.get(id).cloned() {
            return Ok(Some(model));
        }
        infer_oauth_model_from_id(id)
    }

    pub(super) fn toolset(&self, key: &str) -> Option<DynToolset> {
        self.toolsets_by_key.get(key).cloned()
    }

    /// Resolve a registered toolset by id, name, or host alias.
    #[must_use]
    pub fn resolve_toolset(&self, key: &str) -> Option<DynToolset> {
        self.toolset(key)
    }

    pub(super) fn subagent(&self, name: &str) -> Option<SubagentConfig> {
        self.subagents_by_name.get(name).cloned()
    }

    fn register_toolset_keys(&mut self, toolset: &DynToolset) {
        self.toolsets_by_key
            .insert(toolset.name().to_string(), toolset.clone());
        if let Some(id) = toolset.id() {
            self.toolsets_by_key.insert(id.to_string(), toolset.clone());
        }
    }
}

fn infer_oauth_model_from_id(
    model_id: &str,
) -> Result<Option<Arc<dyn ModelAdapter>>, AgentSpecError> {
    let Some(rest) = model_id.strip_prefix("oauth@") else {
        return Ok(None);
    };
    let Some((provider_name, model_name)) = rest.split_once(':') else {
        return Err(AgentSpecError::InvalidOAuthModel {
            model_id: model_id.to_string(),
        });
    };
    if provider_name.is_empty() || model_name.is_empty() {
        return Err(AgentSpecError::InvalidOAuthModel {
            model_id: model_id.to_string(),
        });
    }
    let model = starweaver_oauth_provider::infer_oauth_model(provider_name, model_name).map_err(
        |source| AgentSpecError::OAuthModel {
            model_id: model_id.to_string(),
            source,
        },
    )?;
    Ok(Some(Arc::new(model)))
}
