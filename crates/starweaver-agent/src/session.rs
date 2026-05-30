//! SDK session wrapper for context-backed multi-run applications.

use serde_json::Value;
use starweaver_context::{AgentContext, BusMessage, ResumableState};
use starweaver_environment::DynEnvironmentProvider;

use crate::attach_environment;
use starweaver_core::TraceContext;
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentIterResult, AgentResult, AgentStreamRecord,
    AgentStreamResult,
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

    /// Set a serializable state domain value.
    pub fn set_state(&mut self, key: impl Into<String>, value: Value) {
        self.context.state.set(key, value);
    }

    /// Set a persistent note.
    pub fn set_note(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.context.notes.set(key, value);
    }

    /// Enqueue a session message.
    pub fn enqueue_message(&mut self, topic: impl Into<String>, payload: Value) {
        self.context
            .enqueue_message(BusMessage::new(topic, payload));
    }

    /// Attach session metadata.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: Value) {
        self.context.metadata.insert(key.into(), value);
    }

    /// Attach the active environment provider to the session context.
    #[must_use]
    pub fn with_environment(mut self, provider: DynEnvironmentProvider) -> Self {
        attach_environment(&mut self.context, provider);
        self
    }

    /// Replace the active environment provider on the session context.
    pub fn set_environment(&mut self, provider: DynEnvironmentProvider) {
        attach_environment(&mut self.context, provider);
    }

    /// Attach trace correlation context to the session.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.context.set_trace_context(trace_context);
        self
    }

    /// Attach an external traceparent header or trace id to the session.
    #[must_use]
    pub fn with_trace_parent(self, trace_parent: impl Into<String>) -> Self {
        self.with_trace_context(TraceContext::from_trace_parent(trace_parent))
    }

    /// Replace trace correlation context on the session.
    pub fn set_trace_context(&mut self, trace_context: TraceContext) {
        self.context.set_trace_context(trace_context);
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

impl AgentSession {
    /// Run the session agent and collect compact iteration inspection records.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_iter(
        &mut self,
        prompt: impl Into<String>,
    ) -> Result<AgentIterResult, AgentError> {
        self.agent
            .run_with_context_iter(prompt, &mut self.context)
            .await
    }
}
