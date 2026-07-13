//! Shared least-authority tool dependency assembly.

use starweaver_context::{
    AgentContext, AgentContextHandle, ContextMutationHandles, DependencyStore,
};
use starweaver_tools::{ToolDependencyProfile, ToolDependencyRequirements};

/// Dependencies and mutation cells assembled for one tool invocation.
#[derive(Clone, Debug)]
pub struct ToolDependencyAssembly {
    /// Typed dependencies exposed to the tool.
    pub dependencies: DependencyStore,
    /// Full compatibility context handle, present only for the Legacy profile.
    pub legacy_context: Option<AgentContextHandle>,
    /// Isolated mutable context capability cells.
    pub context_mutations: ContextMutationHandles,
    /// Context capability names authorized for this invocation.
    pub authorized_context_capabilities: std::collections::BTreeSet<String>,
}

impl ToolDependencyAssembly {
    /// Apply tool-owned context changes back to the runtime context.
    pub fn apply_to(&self, context: &mut AgentContext) {
        if let Some(handle) = &self.legacy_context {
            absorb_legacy_context(context, &handle.snapshot());
        }
        self.context_mutations
            .apply_to(context, &self.authorized_context_capabilities);
    }
}

/// Assemble one named tool's dependencies from declared requirements and current host grants.
#[must_use]
pub fn assemble_tool_dependencies_for_name(
    context: &AgentContext,
    tool_name: &str,
    requirements: &ToolDependencyRequirements,
    strict_grant: &starweaver_context::ToolCapabilityGrant,
) -> ToolDependencyAssembly {
    let strict_grant = if tool_name.is_empty() {
        strict_grant.clone()
    } else {
        context.tool_capability_grant(tool_name)
    };
    let authorized_host_capabilities = if requirements.profile == ToolDependencyProfile::Strict {
        requirements
            .host_capabilities
            .intersection(&strict_grant.host_capabilities)
            .cloned()
            .collect()
    } else {
        requirements.host_capabilities.clone()
    };
    let authorized_context_capabilities = if requirements.profile == ToolDependencyProfile::Strict {
        requirements
            .context_capabilities
            .intersection(&strict_grant.context_capabilities)
            .cloned()
            .collect()
    } else {
        requirements.context_capabilities.clone()
    };
    let shell_environment_authorized = requirements.shell_environment
        && (requirements.profile != ToolDependencyProfile::Strict
            || strict_grant.shell_environment);
    let context_mutations = ContextMutationHandles::from_context(context);
    let legacy_context = (requirements.profile == ToolDependencyProfile::Legacy)
        .then(|| AgentContextHandle::new(context.clone()));
    let mut dependencies = match requirements.profile {
        ToolDependencyProfile::Legacy => context.tool_dependency_store(),
        ToolDependencyProfile::Filtered => context.filtered_tool_dependency_store(
            &authorized_host_capabilities,
            shell_environment_authorized,
        ),
        ToolDependencyProfile::Strict => context.strict_tool_dependency_store(
            &authorized_host_capabilities,
            shell_environment_authorized,
        ),
    };
    if let Some(handle) = &legacy_context {
        dependencies.insert(handle.clone());
    }
    context_mutations.insert_grants(&mut dependencies, &authorized_context_capabilities);
    ToolDependencyAssembly {
        dependencies,
        legacy_context,
        context_mutations,
        authorized_context_capabilities,
    }
}

fn absorb_legacy_context(context: &mut AgentContext, snapshot: &AgentContext) {
    context.usage.clone_from(&snapshot.usage);
    context.notes.clone_from(&snapshot.notes);
    context.state.clone_from(&snapshot.state);
    context.tools.clone_from(&snapshot.tools);
    context.events.clone_from(&snapshot.events);
    context.messages.clone_from(&snapshot.messages);
    context.metadata.clone_from(&snapshot.metadata);
    context.agent_registry.clone_from(&snapshot.agent_registry);
    context
        .subagent_history
        .clone_from(&snapshot.subagent_history);
    context
        .handoff_message
        .clone_from(&snapshot.handoff_message);
    context
        .runtime
        .context_manage_tool_names
        .clone_from(&snapshot.runtime.context_manage_tool_names);
    context
        .runtime
        .tool_tags
        .clone_from(&snapshot.runtime.tool_tags);
    context
        .runtime
        .wrapper_metadata
        .clone_from(&snapshot.runtime.wrapper_metadata);
}
