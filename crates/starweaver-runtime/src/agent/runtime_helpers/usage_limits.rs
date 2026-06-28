//! Usage limit and context absorption helpers.

use starweaver_context::{AgentContext, AgentContextHandle};

use crate::{
    agent::{Agent, AgentError},
    run::AgentRunState,
};

impl Agent {
    pub(in crate::agent) fn check_before_request(
        &self,
        state: &AgentRunState,
    ) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            limits.check_before_request(&state.usage)?;
        }
        Ok(())
    }

    pub(in crate::agent) fn check_usage(&self, state: &AgentRunState) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            limits.check_usage(&state.usage)?;
        }
        Ok(())
    }

    pub(in crate::agent) fn absorb_tool_context_handle(
        &self,
        state: &mut AgentRunState,
        context: &mut AgentContext,
        handle: &AgentContextHandle,
    ) -> Result<(), AgentError> {
        let mut snapshot = handle.snapshot();
        let usage = snapshot.usage.clone();
        context.usage = usage.clone();
        state.usage = usage;
        context.notes.clone_from(&snapshot.notes);
        context.state.clone_from(&snapshot.state);
        context.task_manager.clone_from(&snapshot.task_manager);
        context.events.clone_from(&snapshot.events);
        context.messages.clone_from(&snapshot.messages);
        context.metadata.clone_from(&snapshot.metadata);
        context
            .deferred_tool_metadata
            .clone_from(&snapshot.deferred_tool_metadata);
        context.agent_registry.clone_from(&snapshot.agent_registry);
        context
            .subagent_history
            .clone_from(&snapshot.subagent_history);
        context
            .handoff_message
            .clone_from(&snapshot.handoff_message);
        context
            .auto_load_files
            .clone_from(&snapshot.auto_load_files);
        context
            .approval_required_tools
            .clone_from(&snapshot.approval_required_tools);
        context
            .approval_required_mcp_servers
            .clone_from(&snapshot.approval_required_mcp_servers);
        context
            .tool_search_loaded_tools
            .clone_from(&snapshot.tool_search_loaded_tools);
        context
            .tool_search_loaded_namespaces
            .clone_from(&snapshot.tool_search_loaded_namespaces);
        context
            .context_manage_tool_names
            .clone_from(&snapshot.context_manage_tool_names);
        context.tool_tags.clone_from(&snapshot.tool_tags);
        context
            .wrapper_metadata
            .clone_from(&snapshot.wrapper_metadata);
        snapshot
            .message_history
            .clone_from(&context.message_history);
        snapshot.run_id.clone_from(&context.run_id);
        snapshot.trace_context.clone_from(&context.trace_context);
        handle.replace(snapshot);
        self.check_usage(state)
    }

    pub(in crate::agent) fn check_tool_calls(
        &self,
        state: &AgentRunState,
        additional_successful_tool_calls: u64,
    ) -> Result<(), AgentError> {
        if let Some(limits) = &self.usage_limits {
            let projected = state
                .usage
                .clone()
                .with_additional_tool_calls(additional_successful_tool_calls);
            limits.check_tool_calls(&projected)?;
        }
        Ok(())
    }
}
