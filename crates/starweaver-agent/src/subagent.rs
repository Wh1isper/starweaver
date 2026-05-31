//! SDK-level subagent protocol.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentContextHandle};
use starweaver_core::{Metadata, SubagentLifecycleEvent, SubagentLifecycleKind, TaskId};
use starweaver_model::ModelMessage;
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentResult, AgentStreamRecord, AgentStreamResult,
};
use starweaver_tools::{typed_tool, DynTool, EmptyToolArgs, ToolContext, ToolError, ToolResult};

use crate::session::AgentSession;

/// Application-level task envelope used for SDK subagent delegation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubagentTask {
    /// Stable task identifier shared across runtime, service, and SDK layers.
    pub id: TaskId,
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
            id: TaskId::new(),
            prompt: prompt.into(),
            metadata: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    /// Set a caller-provided task identifier.
    #[must_use]
    pub fn with_id(mut self, id: TaskId) -> Self {
        self.id = id;
        self
    }

    /// Attach application metadata to the delegated task.
    #[must_use]
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
struct DelegateArgs {
    /// Name of the subagent to delegate to.
    #[serde(alias = "name")]
    subagent_name: String,
    /// The prompt to send to the subagent.
    prompt: String,
    /// Optional agent ID carried into task metadata for host-managed continuation.
    #[serde(default)]
    agent_id: Option<String>,
    /// Optional application metadata for the delegated task.
    #[serde(default)]
    #[schemars(skip)]
    metadata: Option<serde_json::Value>,
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

    /// Return whether there are no registered subagents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.subagents.is_empty()
    }

    /// Return a stable list of registered subagent names.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.subagents
            .iter()
            .map(|subagent| subagent.name.clone())
            .collect()
    }

    /// Return whether a subagent is available for delegation.
    #[must_use]
    pub fn is_available(&self, name: &str) -> bool {
        self.subagent(name).is_some()
    }

    /// Create a typed delegation tool bound to this registry.
    #[must_use]
    pub fn delegate_tool(self: &Arc<Self>) -> DynTool {
        self.delegate_tool_named("delegate")
    }

    /// Create a subagent information tool bound to this registry.
    #[must_use]
    pub fn subagent_info_tool(self: &Arc<Self>) -> DynTool {
        let registry = self.clone();
        Arc::new(typed_tool::<EmptyToolArgs, _, _>(
            "subagent_info",
            Some("List all known subagents and their metadata.".to_string()),
            move |_context: ToolContext, _arguments: EmptyToolArgs| {
                let registry = registry.clone();
                async move {
                    let subagents = registry
                        .subagents
                        .iter()
                        .map(|subagent| {
                            serde_json::json!({
                                "name": &subagent.name,
                                "description": &subagent.description,
                            })
                        })
                        .collect::<Vec<_>>();
                    Ok(ToolResult::new(serde_json::json!({
                        "subagents": subagents,
                    })))
                }
            },
        ))
    }

    /// Create a typed delegation tool bound to this registry with a caller-provided name.
    #[must_use]
    pub fn delegate_tool_named(self: &Arc<Self>, tool_name: impl Into<String>) -> DynTool {
        let registry = self.clone();
        let tool_name = tool_name.into();
        Arc::new(typed_tool::<DelegateArgs, _, _>(
            tool_name.clone(),
            Some("Delegate a task to a registered SDK subagent.".to_string()),
            move |context: ToolContext, arguments: DelegateArgs| {
                let registry = registry.clone();
                let tool_name = tool_name.clone();
                async move {
                    let context_handle =
                        context.dependency::<AgentContextHandle>().ok_or_else(|| {
                            ToolError::Execution {
                                tool: tool_name.clone(),
                                message: "missing AgentContextHandle dependency".to_string(),
                            }
                        })?;
                    let mut parent_context = context_handle.snapshot();
                    let mut metadata = arguments.metadata.unwrap_or_else(|| serde_json::json!({}));
                    if let Some(agent_id) = arguments.agent_id {
                        metadata["agent_id"] = serde_json::json!(agent_id);
                    }
                    let task = SubagentTask::new(arguments.prompt).with_metadata(metadata);
                    let result = registry
                        .delegate_task(&arguments.subagent_name, task, &mut parent_context)
                        .await;
                    context_handle.replace(parent_context);
                    let result = result.map_err(|error| ToolError::Execution {
                        tool: tool_name.clone(),
                        message: error.to_string(),
                    })?;
                    let mut tool_result = ToolResult::new(serde_json::json!({
                        "name": result.name,
                        "task_id": result.task.id.as_str(),
                        "output": result.output(),
                        "usage": result.result.state.usage,
                    }));
                    tool_result
                        .metadata
                        .insert("context_mutated".to_string(), serde_json::json!(true));
                    Ok(tool_result)
                }
            },
        ))
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
        let Some(subagent) = self.subagent(name) else {
            parent_context.publish_event(starweaver_context::AgentEvent::new(
                "subagent_failed",
                serde_json::to_value(
                    SubagentLifecycleEvent::new(
                        SubagentLifecycleKind::Failed,
                        name,
                        task.id.clone(),
                    )
                    .with_metadata(serde_json::json!({"error": "missing_subagent"})),
                )
                .unwrap_or_else(|_| serde_json::json!({"name": name})),
            ));
            return Err(AgentError::Capability(format!("missing subagent {name}")));
        };
        parent_context.publish_event(starweaver_context::AgentEvent::new(
            "subagent_started",
            serde_json::to_value(
                SubagentLifecycleEvent::new(SubagentLifecycleKind::Started, name, task.id.clone())
                    .with_metadata(task.metadata.clone()),
            )
            .unwrap_or_else(|_| serde_json::json!({"name": name})),
        ));
        let mut child_context = parent_context.subagent_context(name);
        let result = match subagent
            .agent
            .run_with_context(task.prompt.clone(), &mut child_context)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let mut metadata = Metadata::default();
                metadata.insert("error".to_string(), serde_json::json!(error.to_string()));
                if let Some(run_id) = child_context.run_id.clone() {
                    metadata.insert(
                        "child_run_id".to_string(),
                        serde_json::json!(run_id.as_str()),
                    );
                }
                parent_context.absorb_subagent_context(&child_context);
                parent_context.publish_event(starweaver_context::AgentEvent::new(
                    "subagent_failed",
                    serde_json::to_value(
                        SubagentLifecycleEvent::new(
                            SubagentLifecycleKind::Failed,
                            name,
                            task.id.clone(),
                        )
                        .with_run_id(child_context.run_id.clone().unwrap_or_default())
                        .with_metadata(serde_json::Value::Object(metadata)),
                    )
                    .unwrap_or_else(|_| serde_json::json!({"name": name})),
                ));
                return Err(error);
            }
        };
        parent_context.absorb_subagent_context(&child_context);
        parent_context.publish_event(starweaver_context::AgentEvent::new(
            "subagent_completed",
            serde_json::to_value(
                SubagentLifecycleEvent::new(
                    SubagentLifecycleKind::Completed,
                    name,
                    task.id.clone(),
                )
                .with_run_id(result.state.run_id.clone())
                .with_metadata(task.metadata.clone()),
            )
            .unwrap_or_else(|_| serde_json::json!({"name": name})),
        ));
        Ok(SubagentResult {
            name: name.to_string(),
            task,
            result,
        })
    }
}
