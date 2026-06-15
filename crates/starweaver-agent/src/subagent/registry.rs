use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentContextHandle};
use starweaver_core::{Metadata, SubagentLifecycleEvent, SubagentLifecycleKind};
use starweaver_runtime::{AgentError, AgentResult};
use starweaver_tools::{
    typed_json_tool, DynTool, EmptyToolArgs, ToolContext, ToolError, ToolRegistry, ToolResult,
};

use super::{SubagentConfig, SubagentResult, SubagentTask};

const SUBAGENT_STACK_KEY: &str = "starweaver.subagent_stack";

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
        Arc::new(typed_json_tool::<EmptyToolArgs, _, _>(
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
        Arc::new(typed_json_tool::<DelegateArgs, _, _>(
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
                    if let Some(parent_tools) = context.dependency::<SubagentParentTools>() {
                        parent_context
                            .dependencies
                            .insert(parent_tools.as_ref().clone());
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
    #[allow(clippy::too_many_lines)]
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
        if !subagent.tool_inheritance.allow_nested_delegation {
            let stack = current_subagent_stack(parent_context);
            if stack.iter().any(|active| active == name) {
                parent_context.publish_event(starweaver_context::AgentEvent::new(
                    "subagent_failed",
                    serde_json::to_value(
                        SubagentLifecycleEvent::new(
                            SubagentLifecycleKind::Failed,
                            name,
                            task.id.clone(),
                        )
                        .with_metadata(
                            serde_json::json!({"error": "recursive_subagent_delegation"}),
                        ),
                    )
                    .unwrap_or_else(|_| serde_json::json!({"name": name})),
                ));
                return Err(AgentError::Capability(format!(
                    "recursive subagent delegation for {name}"
                )));
            }
        }
        let inherited_tools = parent_context
            .dependency::<SubagentParentTools>()
            .map_or_else(ToolRegistry::new, |tools| tools.0.clone());
        let inherited_tools = subagent
            .tool_inheritance
            .resolve(&inherited_tools)
            .map_err(|error| AgentError::Capability(error.to_string()))?;
        let child_agent_id = task
            .metadata
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| format!("{}-{}", name, task.id.as_str()), str::to_string);
        let mut child_context = parent_context.subagent_context_with_agent_id(name, child_agent_id);
        push_subagent_stack(&mut child_context, name);
        let child_agent = subagent
            .agent
            .as_ref()
            .clone()
            .with_appended_tools(&inherited_tools);
        let result = match child_agent
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
                parent_context.publish_event(starweaver_context::AgentEvent::new(
                    "usage_snapshot",
                    serde_json::to_value(parent_context.build_usage_snapshot())
                        .unwrap_or_else(|_| serde_json::json!({})),
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
        parent_context.publish_event(starweaver_context::AgentEvent::new(
            "usage_snapshot",
            serde_json::to_value(parent_context.build_usage_snapshot())
                .unwrap_or_else(|_| serde_json::json!({})),
        ));
        Ok(SubagentResult {
            name: name.to_string(),
            task,
            result,
        })
    }
}

/// Parent tool registry dependency used to resolve subagent inherited tools.
#[derive(Clone)]
pub struct SubagentParentTools(pub ToolRegistry);

fn current_subagent_stack(context: &AgentContext) -> Vec<String> {
    context
        .metadata
        .get(SUBAGENT_STACK_KEY)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn push_subagent_stack(context: &mut AgentContext, name: &str) {
    let mut stack = current_subagent_stack(context);
    stack.push(name.to_string());
    context
        .metadata
        .insert(SUBAGENT_STACK_KEY.to_string(), serde_json::json!(stack));
}
