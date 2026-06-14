use starweaver_context::AgentContext;
use starweaver_model::ModelMessage;
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentResult, AgentStreamRecord, AgentStreamResult,
};

use crate::session::AgentSession;

use super::SubagentRegistry;

/// SDK application wrapper that keeps application protocols above the core runtime.
#[derive(Clone)]
pub struct AgentApp {
    agent: RuntimeAgent,
    subagents: SubagentRegistry,
}

impl AgentApp {
    /// Build an app wrapper from a runtime agent.
    #[must_use]
    pub fn new(agent: RuntimeAgent) -> Self {
        Self {
            agent,
            subagents: SubagentRegistry::new(),
        }
    }

    /// Attach an SDK-level subagent registry.
    #[must_use]
    pub fn with_subagents(mut self, subagents: SubagentRegistry) -> Self {
        self.subagents = subagents;
        self
    }

    /// Return the underlying runtime agent.
    #[must_use]
    pub const fn agent(&self) -> &RuntimeAgent {
        &self.agent
    }

    /// Return the SDK-level subagent registry.
    #[must_use]
    pub const fn subagents(&self) -> &SubagentRegistry {
        &self.subagents
    }

    /// Create a context-backed SDK session with a fresh context.
    #[must_use]
    pub fn session(&self) -> AgentSession {
        AgentSession::new(self.agent.clone())
    }

    /// Create a context-backed SDK session from caller-provided context.
    #[must_use]
    pub fn session_with_context(&self, context: AgentContext) -> AgentSession {
        AgentSession::with_context(self.agent.clone(), context)
    }

    /// Restore a context-backed SDK session from exported state.
    #[must_use]
    pub fn session_from_state(&self, state: starweaver_context::ResumableState) -> AgentSession {
        AgentSession::from_state(self.agent.clone(), state)
    }

    /// Run the underlying runtime agent with a user prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the core runtime fails.
    pub async fn run(&self, prompt: impl Into<String>) -> Result<AgentResult, AgentError> {
        self.agent.run(prompt).await
    }

    /// Run the underlying runtime agent with prior message history.
    ///
    /// # Errors
    ///
    /// Returns an error when the core runtime fails.
    pub async fn run_with_history(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
    ) -> Result<AgentResult, AgentError> {
        self.agent.run_with_history(prompt, message_history).await
    }

    /// Run with a lifecycle-wide context.
    ///
    /// # Errors
    ///
    /// Returns an error when the core runtime fails.
    pub async fn run_with_context(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
    ) -> Result<AgentResult, AgentError> {
        self.agent.run_with_context(prompt, context).await
    }

    /// Run and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the core runtime fails.
    pub async fn run_stream(
        &self,
        prompt: impl Into<String>,
    ) -> Result<AgentStreamResult, AgentError> {
        self.agent.run_stream(prompt).await
    }

    /// Run with an explicit stream event collector.
    ///
    /// # Errors
    ///
    /// Returns an error when the core runtime fails.
    pub async fn run_with_context_and_stream_events(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        self.agent
            .run_with_context_and_stream_events(prompt, context, events)
            .await
    }
}
