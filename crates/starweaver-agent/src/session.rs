//! SDK session wrapper for context-backed multi-run applications.

use serde_json::Value;
use starweaver_context::{AgentContext, BusMessage, ResumableState};
use starweaver_environment::DynEnvironmentProvider;
use starweaver_model::{ModelRequestParameters, ModelSettings};
use starweaver_tools::{DynTool, DynToolset, ToolRegistry};

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

/// Per-run SDK overrides composed over a reusable session agent.
#[derive(Clone, Default)]
pub struct AgentRunOptions {
    instructions: Vec<String>,
    model_settings: Option<ModelSettings>,
    request_params: Option<ModelRequestParameters>,
    tools: ToolRegistry,
    replace_tools: bool,
}

impl AgentRunOptions {
    /// Create empty run options.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an instruction for this run.
    #[must_use]
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.instructions.push(instruction.into());
        self
    }

    /// Override model settings for this run.
    #[must_use]
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.model_settings = Some(settings);
        self
    }

    /// Override provider-neutral request parameters for this run.
    #[must_use]
    pub fn request_params(mut self, params: ModelRequestParameters) -> Self {
        self.request_params = Some(params);
        self
    }

    /// Add one runtime tool for this run.
    #[must_use]
    pub fn tool(mut self, tool: DynTool) -> Self {
        self.tools.insert(tool);
        self
    }

    /// Add one runtime toolset for this run.
    #[must_use]
    pub fn toolset(mut self, toolset: &DynToolset) -> Self {
        self.tools.insert_toolset(toolset);
        self
    }

    /// Add many runtime toolsets for this run.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        for toolset in toolsets {
            self.tools.insert_toolset(&toolset);
        }
        self
    }

    /// Merge tools from another registry into this run.
    #[must_use]
    pub fn append_tool_registry(mut self, tools: &ToolRegistry) -> Self {
        self.tools.insert_registry(tools);
        self
    }

    /// Use run tools as the complete tool registry for this run.
    #[must_use]
    pub const fn replace_tools(mut self) -> Self {
        self.replace_tools = true;
        self
    }

    /// Apply these options to a reusable runtime agent clone.
    #[must_use]
    pub fn apply(self, agent: &RuntimeAgent) -> RuntimeAgent {
        let mut override_builder = agent.override_config();
        if self.replace_tools {
            override_builder = override_builder.with_tools(self.tools);
        } else if !self.tools.is_empty() {
            override_builder = override_builder.append_tools(&self.tools);
        }
        if !self.instructions.is_empty() {
            override_builder = override_builder.append_instructions(self.instructions);
        }
        if let Some(settings) = self.model_settings {
            override_builder = override_builder.model_settings(Some(settings));
        }
        if let Some(params) = self.request_params {
            override_builder = override_builder.request_params(params);
        }
        override_builder.build()
    }
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

    /// Run with per-run SDK overrides composed over the reusable session agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_with_options(
        &mut self,
        prompt: impl Into<String>,
        options: AgentRunOptions,
    ) -> Result<AgentResult, AgentError> {
        options
            .apply(&self.agent)
            .run_with_context(prompt, &mut self.context)
            .await
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

    /// Run with per-run SDK overrides and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_stream_with_options(
        &mut self,
        prompt: impl Into<String>,
        options: AgentRunOptions,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::<AgentStreamRecord>::new();
        let result = options
            .apply(&self.agent)
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

    /// Run with per-run SDK overrides and collect compact iteration inspection records.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_iter_with_options(
        &mut self,
        prompt: impl Into<String>,
        options: AgentRunOptions,
    ) -> Result<AgentIterResult, AgentError> {
        options
            .apply(&self.agent)
            .run_with_context_iter(prompt, &mut self.context)
            .await
    }
}
