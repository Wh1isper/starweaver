//! SDK-level subagent protocol.

use std::sync::Arc;

use starweaver_context::AgentContext;
use starweaver_model::ModelMessage;
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentResult, AgentStreamRecord, AgentStreamResult,
};

/// Application-level task envelope used for SDK subagent delegation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentTask {
    /// Prompt delegated to the subagent.
    pub prompt: String,
    /// Application metadata carried with the delegated task.
    pub metadata: serde_json::Value,
}

impl SubagentTask {
    /// Build a subagent task from a prompt.
    #[must_use]
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Attach application metadata to the delegated task.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Application-level result envelope returned by SDK subagent delegation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentResult {
    /// Subagent name used for delegation.
    pub name: String,
    /// Delegated task envelope.
    pub task: SubagentTask,
    /// Runtime result produced by the delegated subagent.
    pub result: AgentResult,
}

impl SubagentResult {
    /// Return the final text output produced by the subagent.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.result.output
    }

    /// Consume the envelope and return the underlying runtime result.
    #[must_use]
    pub fn into_result(self) -> AgentResult {
        self.result
    }
}

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

/// Registered subagent configuration for SDK-level delegation.
#[derive(Clone)]
pub struct SubagentConfig {
    /// Subagent name exposed to application delegation policies.
    pub name: String,
    /// Optional subagent description.
    pub description: Option<String>,
    /// Nested agent runtime.
    pub agent: Arc<RuntimeAgent>,
}

impl SubagentConfig {
    /// Build a subagent configuration.
    #[must_use]
    pub fn new(name: impl Into<String>, agent: Arc<RuntimeAgent>) -> Self {
        Self {
            name: name.into(),
            description: None,
            agent,
        }
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// Application-level subagent registry.
#[derive(Clone, Default)]
pub struct SubagentRegistry {
    subagents: Vec<SubagentConfig>,
}

impl SubagentRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one subagent.
    #[must_use]
    pub fn with_subagent(mut self, subagent: SubagentConfig) -> Self {
        self.subagents.push(subagent);
        self
    }

    /// Insert one subagent.
    pub fn insert(&mut self, subagent: SubagentConfig) {
        self.subagents.push(subagent);
    }

    /// Return registered subagents.
    #[must_use]
    pub fn subagents(&self) -> &[SubagentConfig] {
        &self.subagents
    }

    /// Return a subagent by name.
    #[must_use]
    pub fn subagent(&self, name: &str) -> Option<&SubagentConfig> {
        self.subagents.iter().find(|subagent| subagent.name == name)
    }

    /// Run a named subagent while sharing usage and dependencies with the parent context.
    ///
    /// # Errors
    ///
    /// Returns an error when the subagent is missing or the nested agent run fails.
    pub async fn delegate(
        &self,
        name: &str,
        prompt: impl Into<String>,
        parent_context: &mut AgentContext,
    ) -> Result<AgentResult, AgentError> {
        self.delegate_task(name, SubagentTask::new(prompt), parent_context)
            .await
            .map(SubagentResult::into_result)
    }

    /// Run a named subagent with an application-level task envelope.
    ///
    /// # Errors
    ///
    /// Returns an error when the subagent is missing or the nested agent run fails.
    pub async fn delegate_task(
        &self,
        name: &str,
        task: SubagentTask,
        parent_context: &mut AgentContext,
    ) -> Result<SubagentResult, AgentError> {
        let subagent = self
            .subagent(name)
            .ok_or_else(|| AgentError::Capability(format!("missing subagent {name}")))?;
        let mut child_context = AgentContext {
            usage: parent_context.usage.clone(),
            dependencies: parent_context.dependencies.clone(),
            ..AgentContext::default()
        };
        let result = subagent
            .agent
            .run_with_context(task.prompt.clone(), &mut child_context)
            .await?;
        parent_context.usage = result.state.usage.clone();
        parent_context.publish_event(starweaver_context::AgentEvent::new(
            "subagent_complete",
            serde_json::json!({
                "name": name,
                "run_id": result.state.run_id.as_str(),
                "task": task.metadata,
            }),
        ));
        Ok(SubagentResult {
            name: name.to_string(),
            task,
            result,
        })
    }
}
