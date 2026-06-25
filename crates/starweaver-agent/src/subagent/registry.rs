use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent};
use starweaver_core::{Metadata, SubagentLifecycleEvent, SubagentLifecycleKind, TaskId};
use starweaver_runtime::{
    AgentCapability, AgentError, AgentResult, AgentStreamRecord, AgentStreamSink,
    AgentStreamSource, CapabilityBundle, TraceRecorderHandle,
};
use starweaver_tools::{
    typed_json_tool, DynTool, EmptyToolArgs, ToolContext, ToolError, ToolRegistry, ToolResult,
};

use crate::bundles::attach_environment;

use super::{
    SubagentConfig, SubagentExecutionMetadata, SubagentExecutionOutcome, SubagentResult,
    SubagentTask, SubagentToolInheritanceError,
};

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

    pub(crate) fn with_resolved_capability_inheritance(
        mut self,
        parent_capabilities: &[Arc<dyn AgentCapability>],
        parent_capability_bundles: &[Arc<dyn CapabilityBundle>],
    ) -> Self {
        self.subagents = self
            .subagents
            .into_iter()
            .map(|subagent| {
                subagent.with_resolved_capability_inheritance(
                    parent_capabilities,
                    parent_capability_bundles,
                )
            })
            .collect();
        self
    }

    /// Return registered subagents.
    #[must_use]
    pub fn subagents(&self) -> &[SubagentConfig] {
        &self.subagents
    }

    /// Return whether there are no registered subagents.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
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
            move |context: ToolContext, _arguments: EmptyToolArgs| {
                let registry = registry.clone();
                async move {
                    let parent_tools = context.dependency::<SubagentParentTools>();
                    let subagents = registry
                        .subagents
                        .iter()
                        .map(|subagent| {
                            let mut payload = serde_json::json!({
                                "name": &subagent.name,
                                "description": &subagent.description,
                            });
                            if let Some(parent_tools) = parent_tools.as_ref() {
                                attach_subagent_availability(
                                    &mut payload,
                                    subagent,
                                    &parent_tools.0,
                                );
                            }
                            payload
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
                    parent_context.trace_context = context.trace_context.clone();
                    if let Some(trace_recorder) = context.dependency::<TraceRecorderHandle>() {
                        parent_context
                            .dependencies
                            .insert(trace_recorder.as_ref().clone());
                    }
                    let mut metadata = arguments.metadata.unwrap_or_else(|| serde_json::json!({}));
                    if let Some(agent_id) = arguments.agent_id {
                        metadata["agent_id"] = serde_json::json!(agent_id);
                    }
                    if let Some(parent_tools) = context.dependency::<SubagentParentTools>() {
                        parent_context
                            .dependencies
                            .insert(parent_tools.as_ref().clone());
                    }
                    let stream_sink = context.dependency::<AgentStreamSink>();
                    let task = SubagentTask::new(arguments.prompt).with_metadata(metadata);
                    let result = Box::pin(registry.delegate_task_with_stream_sink(
                        &arguments.subagent_name,
                        task,
                        &mut parent_context,
                        stream_sink,
                    ))
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
        Box::pin(self.delegate_task(name, SubagentTask::new(prompt), parent_context))
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
        Box::pin(self.delegate_task_with_stream_sink(name, task, parent_context, None)).await
    }

    #[allow(clippy::too_many_lines)]
    async fn delegate_task_with_stream_sink(
        &self,
        name: &str,
        task: SubagentTask,
        parent_context: &mut AgentContext,
        stream_sink: Option<Arc<AgentStreamSink>>,
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
        let inherited_tools = match subagent.tool_inheritance.resolve(&inherited_tools) {
            Ok(inherited_tools) => inherited_tools,
            Err(error) => {
                publish_subagent_failed(
                    parent_context,
                    name,
                    &task.id,
                    None,
                    tool_inheritance_diagnostic(&error),
                );
                return Err(AgentError::Capability(error.to_string()));
            }
        };
        let child_agent_id = task
            .metadata
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| format!("{}-{}", name, task.id.as_str()), str::to_string);
        let mut child_context = parent_context.subagent_context_with_agent_id(name, child_agent_id);
        if let Some(environment) = subagent.environment_provider() {
            attach_environment(&mut child_context, environment);
        }
        push_subagent_stack(&mut child_context, name);
        let execution_metadata =
            SubagentExecutionMetadata::new(name, &task, parent_context, &child_context);
        for hook in &subagent.execution_hooks {
            if let Err(error) = hook
                .before_subagent_run(execution_metadata.clone(), &mut child_context)
                .await
            {
                publish_subagent_failed(
                    parent_context,
                    name,
                    &task.id,
                    child_context.run_id.clone(),
                    serde_json::json!({
                        "error": error.to_string(),
                        "hook": "before_subagent_run"
                    }),
                );
                return Err(error);
            }
        }
        let child_agent = subagent
            .agent
            .as_ref()
            .clone()
            .with_appended_tools(&inherited_tools);
        let mut child_agent = child_agent;
        if let Some(trace_recorder) = parent_context.dependency::<TraceRecorderHandle>() {
            child_agent = child_agent.with_trace_recorder(trace_recorder.recorder());
        }
        for capability in &subagent.inherited_capabilities {
            child_agent = child_agent.with_capability(capability.clone());
        }
        for bundle in &subagent.inherited_capability_bundles {
            child_agent = child_agent.with_capability_bundle(bundle.as_ref());
        }
        let mut child_stream_records = Vec::new();
        let result = match child_agent
            .run_with_context_and_stream_events(
                task.prompt.clone(),
                &mut child_context,
                &mut child_stream_records,
            )
            .await
        {
            Ok(result) => result,
            Err(error) => {
                let outcome = SubagentExecutionOutcome::Failed {
                    error: error.to_string(),
                    run_id: child_context.run_id.clone(),
                };
                for hook in &subagent.execution_hooks {
                    let _ = hook
                        .after_subagent_run(
                            execution_metadata.clone(),
                            &child_context,
                            outcome.clone(),
                        )
                        .await;
                }
                let mut metadata = Metadata::default();
                metadata.insert("error".to_string(), serde_json::json!(error.to_string()));
                if let Some(run_id) = child_context.run_id.clone() {
                    metadata.insert(
                        "child_run_id".to_string(),
                        serde_json::json!(run_id.as_str()),
                    );
                }
                parent_context.absorb_subagent_context(&child_context);
                publish_subagent_stream_records(
                    parent_context,
                    name,
                    &task.id,
                    &child_context,
                    &child_stream_records,
                    stream_sink.as_deref(),
                );
                publish_subagent_failed(
                    parent_context,
                    name,
                    &task.id,
                    child_context.run_id.clone(),
                    serde_json::Value::Object(metadata),
                );
                parent_context.publish_event(starweaver_context::AgentEvent::new(
                    "usage_snapshot",
                    serde_json::to_value(parent_context.build_usage_snapshot())
                        .unwrap_or_else(|_| serde_json::json!({})),
                ));
                return Err(error);
            }
        };
        let outcome = SubagentExecutionOutcome::Completed {
            output: result.output.clone(),
            run_id: Some(result.state.run_id.clone()),
            usage: result.state.usage.clone(),
        };
        for hook in &subagent.execution_hooks {
            if let Err(error) = hook
                .after_subagent_run(execution_metadata.clone(), &child_context, outcome.clone())
                .await
            {
                parent_context.absorb_subagent_context(&child_context);
                publish_subagent_stream_records(
                    parent_context,
                    name,
                    &task.id,
                    &child_context,
                    &child_stream_records,
                    stream_sink.as_deref(),
                );
                publish_subagent_failed(
                    parent_context,
                    name,
                    &task.id,
                    child_context.run_id.clone(),
                    serde_json::json!({
                        "error": error.to_string(),
                        "hook": "after_subagent_run"
                    }),
                );
                return Err(error);
            }
        }
        parent_context.absorb_subagent_context(&child_context);
        publish_subagent_stream_records(
            parent_context,
            name,
            &task.id,
            &child_context,
            &child_stream_records,
            stream_sink.as_deref(),
        );
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

fn publish_subagent_stream_records(
    parent_context: &mut AgentContext,
    name: &str,
    task_id: &TaskId,
    child_context: &AgentContext,
    records: &[AgentStreamRecord],
    stream_sink: Option<&AgentStreamSink>,
) {
    if let Some(stream_sink) = stream_sink {
        stream_sink.extend(records.iter().map(|record| {
            record.clone().with_source(AgentStreamSource::subagent(
                child_context.agent_id.clone(),
                name,
                task_id.clone(),
                child_context.run_id.clone(),
                child_context.parent_run_id.clone(),
                record.sequence,
            ))
        }));
        return;
    }

    let child_run_id = child_context
        .run_id
        .as_ref()
        .map(starweaver_core::RunId::as_str);
    for record in records {
        let record_value = serde_json::to_value(record).unwrap_or_else(|_| serde_json::json!({}));
        let event_kind = record_value
            .get("event")
            .and_then(|event| event.get("kind"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let payload = serde_json::json!({
            "name": name,
            "task_id": task_id.as_str(),
            "source_agent_id": child_context.agent_id.as_str(),
            "source_agent_name": name,
            "source_run_id": child_run_id,
            "source_sequence": record.sequence,
            "source_event_kind": event_kind,
            "record": record_value,
        });
        let mut metadata = Metadata::default();
        metadata.insert("subagent_name".to_string(), serde_json::json!(name));
        metadata.insert("task_id".to_string(), serde_json::json!(task_id.as_str()));
        metadata.insert(
            "source_agent_id".to_string(),
            serde_json::json!(child_context.agent_id.as_str()),
        );
        if let Some(run_id) = child_run_id {
            metadata.insert("source_run_id".to_string(), serde_json::json!(run_id));
        }
        metadata.insert(
            "source_sequence".to_string(),
            serde_json::json!(record.sequence),
        );
        parent_context.publish_event(
            AgentEvent::new("subagent_stream_record", payload).with_metadata(metadata),
        );
    }
}

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

fn attach_subagent_availability(
    payload: &mut serde_json::Value,
    subagent: &SubagentConfig,
    parent_tools: &ToolRegistry,
) {
    let Some(payload) = payload.as_object_mut() else {
        return;
    };
    match subagent.tool_inheritance.resolve(parent_tools) {
        Ok(inherited) => {
            payload.insert("available".to_string(), serde_json::json!(true));
            payload.insert(
                "inherited_tools".to_string(),
                serde_json::json!(inherited.names()),
            );
            payload.insert(
                "diagnostics".to_string(),
                serde_json::Value::Array(Vec::new()),
            );
        }
        Err(error) => {
            payload.insert("available".to_string(), serde_json::json!(false));
            payload.insert(
                "diagnostics".to_string(),
                serde_json::json!([tool_inheritance_diagnostic(&error)]),
            );
        }
    }
}

fn publish_subagent_failed(
    context: &mut AgentContext,
    name: &str,
    task_id: &TaskId,
    run_id: Option<starweaver_core::RunId>,
    metadata: serde_json::Value,
) {
    let mut event =
        SubagentLifecycleEvent::new(SubagentLifecycleKind::Failed, name, task_id.clone())
            .with_metadata(metadata);
    if let Some(run_id) = run_id {
        event = event.with_run_id(run_id);
    }
    context.publish_event(starweaver_context::AgentEvent::new(
        "subagent_failed",
        serde_json::to_value(event).unwrap_or_else(|_| serde_json::json!({"name": name})),
    ));
}

fn tool_inheritance_diagnostic(error: &SubagentToolInheritanceError) -> serde_json::Value {
    match error {
        SubagentToolInheritanceError::MissingRequiredTool(tool_name) => serde_json::json!({
            "error": "missing_required_tool",
            "error_kind": "missing_required_tool",
            "tool_name": tool_name,
            "message": error.to_string(),
        }),
        SubagentToolInheritanceError::DeniedRequiredTool(tool_name) => serde_json::json!({
            "error": "denied_required_tool",
            "error_kind": "denied_required_tool",
            "tool_name": tool_name,
            "message": error.to_string(),
        }),
    }
}
