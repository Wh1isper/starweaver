//! Usage limit and context absorption helpers.

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
