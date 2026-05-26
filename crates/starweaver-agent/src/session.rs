//! SDK session wrapper for context-backed multi-run applications.

use starweaver_context::{AgentContext, ResumableState};
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentResult, AgentStreamRecord, AgentStreamResult,
};

/// Context-backed SDK session for repeated runs through one agent.
#[derive(Clone)]
pub struct AgentSession {
    agent: RuntimeAgent,
    context: AgentContext,
}

impl AgentSession {
    /// Create a session from a runtime agent and a fresh context.
    #[must_use]
    pub fn new(agent: RuntimeAgent) -> Self {
        Self::with_context(agent, AgentContext::default())
    }

    /// Create a session from a runtime agent and caller-provided context.
    #[must_use]
    pub const fn with_context(agent: RuntimeAgent, context: AgentContext) -> Self {
        Self { agent, context }
    }

    /// Restore a session from exported context state.
    #[must_use]
    pub fn from_state(agent: RuntimeAgent, state: ResumableState) -> Self {
        Self::with_context(agent, AgentContext::from_state(state))
    }

    /// Return the underlying runtime agent.
    #[must_use]
    pub const fn agent(&self) -> &RuntimeAgent {
        &self.agent
    }

    /// Return the session context.
    #[must_use]
    pub const fn context(&self) -> &AgentContext {
        &self.context
    }

    /// Return the mutable session context.
    #[must_use]
    pub fn context_mut(&mut self) -> &mut AgentContext {
        &mut self.context
    }

    /// Export session state for later restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        self.context.export_state()
    }

    /// Run the session agent with the session context.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run(&mut self, prompt: impl Into<String>) -> Result<AgentResult, AgentError> {
        self.agent.run_with_context(prompt, &mut self.context).await
    }

    /// Run the session agent and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_stream(
        &mut self,
        prompt: impl Into<String>,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::<AgentStreamRecord>::new();
        let result = self
            .agent
            .run_with_context_and_stream_events(prompt, &mut self.context, &mut events)
            .await?;
        Ok(AgentStreamResult { result, events })
    }
}
