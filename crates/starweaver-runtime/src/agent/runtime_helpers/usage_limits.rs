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
        context.events.clone_from(&snapshot.events);
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
