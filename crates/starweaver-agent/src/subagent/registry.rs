use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent, BusMessage};
use starweaver_core::{
    AgentId, Metadata, SubagentAttemptId, SubagentLifecycleEvent, SubagentLifecycleKind, TaskId,
    escape_xml_attribute, escape_xml_text,
};
use starweaver_runtime::{
    AgentCapability, AgentError, AgentResult, AgentRunState, AgentStreamRecord, AgentStreamSink,
    AgentStreamSource, CapabilityBundle, CapabilityResult, CapabilitySpec, TraceRecorderHandle,
};
use starweaver_tools::{
    DynTool, EmptyToolArgs, TOOL_METADATA_DEPENDENCIES_KEY, ToolContext,
    ToolDependencyRequirements, ToolError, ToolInstruction, ToolRegistry, ToolResult,
    typed_json_tool,
};
use uuid::Uuid;

use crate::bundles::attach_environment;

use super::supervisor::{
    BackgroundSubagentAcceptance, BackgroundSubagentChildControl, BackgroundSubagentContextDelta,
};
use super::{
    BackgroundSubagentDeliveryClaim, BackgroundSubagentError, BackgroundSubagentExecutionStatus,
    BackgroundSubagentMonitor, BackgroundSubagentTaskResult, SubagentConfig,
    SubagentExecutionMetadata, SubagentExecutionOutcome, SubagentResult, SubagentTask,
    SubagentToolInheritanceError,
};

const SUBAGENT_STACK_KEY: &str = "starweaver.subagent_stack";

/// Hidden delegate backend tool used by async delegation wrappers.
pub const DELEGATE_BACKEND_TOOL_NAME: &str = "__delegate_backend";

/// Tool name for explicit background delegation when blocking delegate remains visible.
pub const SPAWN_DELEGATE_TOOL_NAME: &str = "spawn_delegate";

/// Tool name for bounded background subagent fan-in.
pub const WAIT_SUBAGENT_TOOL_NAME: &str = "wait_subagent";

/// Tool name for targeted active-attempt steering.
pub const STEER_SUBAGENT_TOOL_NAME: &str = "steer_subagent";

/// Tool name for targeted cooperative cancellation.
pub const CANCEL_SUBAGENT_TOOL_NAME: &str = "cancel_subagent";

const BACKGROUND_SUBAGENT_CAPABILITY_ID: &str = "starweaver.subagent.background";

fn dependency_metadata(requirements: &ToolDependencyRequirements) -> Metadata {
    Metadata::from_iter([(
        TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
        requirements.to_metadata_value(),
    )])
}

fn explicit_delegation_legacy_metadata() -> Metadata {
    dependency_metadata(
        &ToolDependencyRequirements::legacy_with_context_capabilities(std::iter::empty::<String>()),
    )
}

fn filtered_read_only_metadata() -> Metadata {
    dependency_metadata(&ToolDependencyRequirements::filtered(
        std::iter::empty::<String>(),
        false,
    ))
}

/// Model-visible subagent delegation topology.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SubagentDelegationMode {
    /// Install no model-visible or hidden delegation tools.
    Disabled,
    /// Expose `delegate` as a blocking tool.
    #[default]
    Blocking,
    /// Expose `delegate` as an asynchronous background tool backed by hidden `__delegate_backend`.
    Async,
    /// Expose blocking `delegate` plus explicit `spawn_delegate`.
    BlockingAndAsync,
}

impl SubagentDelegationMode {
    /// Return whether any delegation tools are installed.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::Disabled)
    }

    /// Return whether this mode exposes blocking `delegate`.
    #[must_use]
    pub const fn exposes_blocking_delegate(self) -> bool {
        matches!(self, Self::Blocking | Self::BlockingAndAsync)
    }

    /// Return whether this mode exposes asynchronous `delegate`.
    #[must_use]
    pub const fn exposes_async_delegate(self) -> bool {
        matches!(self, Self::Async)
    }

    /// Return whether this mode exposes explicit `spawn_delegate`.
    #[must_use]
    pub const fn exposes_spawn_delegate(self) -> bool {
        matches!(self, Self::BlockingAndAsync)
    }

    /// Return whether this mode needs a background monitor.
    #[must_use]
    pub const fn needs_background_monitor(self) -> bool {
        matches!(self, Self::Async | Self::BlockingAndAsync)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DelegateArgs {
    /// Name of the subagent to delegate to.
    #[serde(alias = "name")]
    subagent_name: String,
    /// The prompt to send to the subagent.
    prompt: String,
    /// Optional agent ID carried into task metadata for host-managed continuation.
    #[serde(default)]
    agent_id: Option<String>,
    /// Optional separate task-bundle work item linked to this attempt.
    #[serde(default)]
    linked_task_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct BlockingDelegateArgs {
    #[serde(alias = "name")]
    subagent_name: String,
    prompt: String,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    linked_task_id: Option<String>,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct WaitSubagentArgs {
    /// Optional background attempt to wait for. Omit to wait for the current known set.
    #[serde(default)]
    attempt_id: Option<String>,
    /// Maximum seconds to wait before returning without cancelling the subagent.
    #[serde(default = "default_wait_subagent_timeout_seconds")]
    timeout_seconds: f64,
}

const fn default_wait_subagent_timeout_seconds() -> f64 {
    30.0
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct SteerSubagentArgs {
    /// Active attempt to steer.
    attempt_id: String,
    /// Guidance queued for the child's next control boundary.
    message: String,
    /// Optional stable idempotency id.
    #[serde(default)]
    steering_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CancelSubagentArgs {
    /// Active attempt to cancel.
    attempt_id: String,
    /// Optional safe cancellation reason.
    #[serde(default)]
    reason: Option<String>,
    /// Optional stable idempotency id.
    #[serde(default)]
    cancellation_id: Option<String>,
}

/// Runtime hook that redelivers completed background subagent messages.
#[derive(Clone)]
pub struct BackgroundSubagentCapability {
    monitor: Arc<BackgroundSubagentMonitor>,
}

impl BackgroundSubagentCapability {
    /// Create a capability bound to a shared monitor.
    #[must_use]
    pub const fn new(monitor: Arc<BackgroundSubagentMonitor>) -> Self {
        Self { monitor }
    }

    fn drain_into_context(&self, context: &mut AgentContext) {
        self.monitor.apply_context_deltas(context);
        for message in self.monitor.drain_pending_messages() {
            let agent_id = context.agent_id.as_str().to_string();
            context.messages.subscribe(agent_id);
            context.send_message(message);
        }
    }
}

#[async_trait::async_trait]
impl AgentCapability for BackgroundSubagentCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new(BACKGROUND_SUBAGENT_CAPABILITY_ID)
    }

    async fn on_run_start_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
    ) -> CapabilityResult<()> {
        context.dependencies.insert_arc(self.monitor.clone());
        self.drain_into_context(context);
        Ok(())
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<starweaver_model::ModelMessage>,
    ) -> CapabilityResult<Vec<starweaver_model::ModelMessage>> {
        context.dependencies.insert_arc(self.monitor.clone());
        self.drain_into_context(context);
        Ok(messages)
    }

    async fn before_tool_execution_with_context(
        &self,
        _state: &mut AgentRunState,
        _context: &mut AgentContext,
        tool_context: &mut ToolContext,
        _call: &starweaver_model::ToolCallPart,
    ) -> CapabilityResult<()> {
        tool_context.dependencies.insert_arc(self.monitor.clone());
        Ok(())
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

    /// Create a hidden blocking delegate backend for async wrappers.
    #[must_use]
    pub fn hidden_delegate_backend_tool(self: &Arc<Self>) -> DynTool {
        self.delegate_tool_named_with_visibility(DELEGATE_BACKEND_TOOL_NAME, false)
    }

    /// Create an asynchronous background delegate tool named `delegate`.
    #[must_use]
    pub fn async_delegate_tool(
        self: &Arc<Self>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        self.background_delegate_tool_named(
            "delegate",
            "Delegate task to a registered SDK subagent asynchronously. Do not manually poll or loop; use wait_subagent once with a bounded timeout only when you need the result before continuing.",
            monitor,
        )
    }

    /// Create an explicit asynchronous background delegate tool named `spawn_delegate`.
    #[must_use]
    pub fn spawn_delegate_tool(
        self: &Arc<Self>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        self.background_delegate_tool_named(
            SPAWN_DELEGATE_TOOL_NAME,
            "Spawn a registered SDK subagent in the background. Do not manually poll or loop; the result arrives via message bus unless consumed by wait_subagent.",
            monitor,
        )
    }

    /// Create a bounded fan-in tool for background subagent results.
    #[must_use]
    pub fn wait_subagent_tool(
        self: &Arc<Self>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        Arc::new(
            typed_json_tool::<WaitSubagentArgs, _, _>(
                WAIT_SUBAGENT_TOOL_NAME,
                Some(
                    "Wait for one or more background subagents to finish and return their cached results."
                        .to_string(),
                ),
                move |context: ToolContext, arguments: WaitSubagentArgs| {
                    let monitor = monitor.clone();
                    async move {
                        let context_handle = context.dependency::<AgentContextHandle>().ok_or_else(|| {
                            ToolError::UserError {
                                tool: WAIT_SUBAGENT_TOOL_NAME.to_string(),
                                message: "missing AgentContextHandle dependency".to_string(),
                            }
                        })?;
                        let snapshot = context_handle.snapshot();
                        if snapshot.parent_run_id.is_some()
                            || snapshot.metadata.contains_key("parent_agent_id")
                            || snapshot.agent_id.as_str() != "main"
                        {
                            return Err(ToolError::UserError {
                                tool: WAIT_SUBAGENT_TOOL_NAME.to_string(),
                                message: "wait_subagent is only available to the main agent".to_string(),
                            });
                        }
                        let timeout = normalize_wait_subagent_timeout(arguments.timeout_seconds);
                        let content = if let Some(attempt_id) = arguments.attempt_id {
                            let attempt_id = SubagentAttemptId::from_string(attempt_id);
                            wait_for_one_background_subagent(
                                &monitor,
                                &context_handle,
                                &attempt_id,
                                timeout,
                                snapshot.agent_id.as_str(),
                            )
                            .await
                        } else {
                            wait_for_all_background_subagents(
                                &monitor,
                                &context_handle,
                                timeout,
                                snapshot.agent_id.as_str(),
                            )
                            .await
                        };
                        Ok(ToolResult::new(content))
                    }
                },
            )
            .with_metadata(explicit_delegation_legacy_metadata())
            .with_tag("delegation")
            .with_prepare_definition(|context, definition| {
                let monitor = context.dependency::<BackgroundSubagentMonitor>()?;
                let main_agent = context.parent_run_id.is_none()
                    && !context.metadata.contains_key("parent_agent_id")
                    && context.agent_id.as_str() == "main";
                (main_agent && (monitor.has_active_tasks() || !monitor.task_results().is_empty()))
                    .then_some(definition)
            }),
        )
    }

    /// Create a targeted steering tool for active owned attempts.
    #[must_use]
    pub fn steer_subagent_tool(
        self: &Arc<Self>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        Arc::new(
            typed_json_tool::<SteerSubagentArgs, _, _>(
                STEER_SUBAGENT_TOOL_NAME,
                Some(
                    "Queue guidance for one active owned background subagent attempt.".to_string(),
                ),
                move |context: ToolContext, arguments: SteerSubagentArgs| {
                    let monitor = monitor.clone();
                    async move {
                        ensure_main_agent_tool(&context, STEER_SUBAGENT_TOOL_NAME)?;
                        let attempt_id = SubagentAttemptId::from_string(arguments.attempt_id);
                        let steering_id = arguments
                            .steering_id
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| format!("steer_{}", Uuid::new_v4()));
                        let receipt = monitor
                            .steer(&attempt_id, arguments.message, steering_id)
                            .await
                            .map_err(|error| {
                                background_tool_error(STEER_SUBAGENT_TOOL_NAME, &error)
                            })?;
                        Ok(ToolResult::new(
                            serde_json::to_value(receipt)
                                .unwrap_or_else(|_| serde_json::json!({"status": "queued"})),
                        ))
                    }
                },
            )
            .with_metadata(explicit_delegation_legacy_metadata())
            .with_tag("delegation")
            .with_prepare_definition(|context, definition| {
                let monitor = context.dependency::<BackgroundSubagentMonitor>()?;
                (is_main_agent_context(context) && monitor.has_active_tasks()).then_some(definition)
            }),
        )
    }

    /// Create a targeted cooperative cancellation tool for active owned attempts.
    #[must_use]
    pub fn cancel_subagent_tool(
        self: &Arc<Self>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        Arc::new(
            typed_json_tool::<CancelSubagentArgs, _, _>(
                CANCEL_SUBAGENT_TOOL_NAME,
                Some(
                    "Request cooperative cancellation of one active owned background subagent attempt."
                        .to_string(),
                ),
                move |context: ToolContext, arguments: CancelSubagentArgs| {
                    let monitor = monitor.clone();
                    async move {
                        ensure_main_agent_tool(&context, CANCEL_SUBAGENT_TOOL_NAME)?;
                        let attempt_id = SubagentAttemptId::from_string(arguments.attempt_id);
                        let cancellation_id = arguments
                            .cancellation_id
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| format!("cancel_{}", Uuid::new_v4()));
                        let receipt = monitor
                            .request_cancellation_with_reason(
                                &attempt_id,
                                cancellation_id,
                                arguments.reason,
                            )
                            .map_err(|error| background_tool_error(
                                CANCEL_SUBAGENT_TOOL_NAME,
                                &error,
                            ))?;
                        Ok(ToolResult::new(
                            serde_json::to_value(receipt).unwrap_or_else(|_| {
                                serde_json::json!({"status": "cancellation_requested"})
                            }),
                        ))
                    }
                },
            )
            .with_metadata(explicit_delegation_legacy_metadata())
            .with_tag("delegation")
            .with_prepare_definition(|context, definition| {
                let monitor = context.dependency::<BackgroundSubagentMonitor>()?;
                (is_main_agent_context(context) && monitor.has_active_tasks())
                    .then_some(definition)
            }),
        )
    }

    /// Create a subagent information tool bound to this registry.
    #[must_use]
    pub fn subagent_info_tool(self: &Arc<Self>) -> DynTool {
        let registry = self.clone();
        Arc::new(
            typed_json_tool::<EmptyToolArgs, _, _>(
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
                        let background = context
                            .dependency::<BackgroundSubagentMonitor>()
                            .map_or_else(
                                || {
                                    serde_json::json!({
                                        "active": [],
                                        "retained": [],
                                    })
                                },
                                |monitor| {
                                    let active = monitor
                                        .active_tasks()
                                        .into_iter()
                                        .map(|info| {
                                            serde_json::json!({
                                                "attempt_id": info.attempt_id,
                                                "agent_id": info.agent_id,
                                                "subagent_name": info.subagent_name,
                                                "linked_task_id": info.linked_task_id,
                                                "status": info.execution_status,
                                                "child_run_id": info.child_run_id,
                                                "is_resume": info.is_resume,
                                                "accepted_at": info.accepted_at,
                                                "updated_at": info.updated_at,
                                            })
                                        })
                                        .collect::<Vec<_>>();
                                    let retained = monitor
                                        .task_results()
                                        .into_values()
                                        .map(|result| {
                                            serde_json::json!({
                                                "attempt_id": result.attempt_id,
                                                "agent_id": result.agent_id,
                                                "subagent_name": result.subagent_name,
                                                "linked_task_id": result.linked_task_id,
                                                "status": result.status,
                                                "child_run_id": result.child_run_id,
                                                "failure_category": result.failure_category,
                                                "delivery_status": result.delivery_status,
                                                "retention_status": result.retention_status,
                                                "completed_at": result.completed_at,
                                            })
                                        })
                                        .collect::<Vec<_>>();
                                    serde_json::json!({
                                        "active": active,
                                        "retained": retained,
                                    })
                                },
                            );
                        Ok(ToolResult::new(serde_json::json!({
                            "subagents": subagents,
                            "background": background,
                        })))
                    }
                },
            )
            .with_metadata(filtered_read_only_metadata()),
        )
    }

    /// Create a typed delegation tool bound to this registry with a caller-provided name.
    #[must_use]
    pub fn delegate_tool_named(self: &Arc<Self>, tool_name: impl Into<String>) -> DynTool {
        self.delegate_tool_named_with_visibility(tool_name, true)
    }

    fn delegate_tool_named_with_visibility(
        self: &Arc<Self>,
        tool_name: impl Into<String>,
        visible: bool,
    ) -> DynTool {
        let registry = self.clone();
        let tool_name = tool_name.into();
        let tool = typed_json_tool::<BlockingDelegateArgs, _, _>(
            tool_name.clone(),
            Some("Delegate a task to a registered SDK subagent.".to_string()),
            move |context: ToolContext, arguments: BlockingDelegateArgs| {
                let registry = registry.clone();
                let tool_name = tool_name.clone();
                async move {
                    let context_handle =
                        context.dependency::<AgentContextHandle>().ok_or_else(|| {
                            ToolError::UserError {
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
                    let mut metadata = match arguments.metadata {
                        serde_json::Value::Object(metadata) => serde_json::Value::Object(metadata),
                        _ => serde_json::json!({}),
                    };
                    if let Some(agent_id) = arguments.agent_id {
                        metadata["agent_id"] = serde_json::json!(agent_id);
                    }
                    if let Some(linked_task_id) = arguments.linked_task_id {
                        metadata["linked_task_id"] = serde_json::json!(linked_task_id);
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
                        None,
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
        )
        .with_metadata(explicit_delegation_legacy_metadata())
        .with_tag("delegation");
        if visible {
            Arc::new(tool)
        } else {
            Arc::new(tool.with_prepare_definition(|_, _| None))
        }
    }

    #[allow(clippy::too_many_lines)]
    fn background_delegate_tool_named(
        self: &Arc<Self>,
        tool_name: impl Into<String>,
        description: impl Into<String>,
        monitor: Arc<BackgroundSubagentMonitor>,
    ) -> DynTool {
        let registry = self.clone();
        let tool_name = tool_name.into();
        let description = description.into();
        Arc::new(
            typed_json_tool::<DelegateArgs, _, _>(
                tool_name.clone(),
                Some(description),
                move |context: ToolContext, arguments: DelegateArgs| {
                    let registry = registry.clone();
                    let monitor = monitor.clone();
                    let tool_name = tool_name.clone();
                    async move {
                        let context_handle =
                            context.dependency::<AgentContextHandle>().ok_or_else(|| {
                                ToolError::UserError {
                                    tool: tool_name.clone(),
                                    message: "missing AgentContextHandle dependency".to_string(),
                                }
                            })?;
                        let parent_context = context_handle.snapshot();
                        if parent_context.parent_run_id.is_some()
                            || parent_context.metadata.contains_key("parent_agent_id")
                        {
                            return Err(ToolError::UserError {
                                tool: tool_name.clone(),
                                message: "background subagent delegation is only available to the main agent"
                                    .to_string(),
                            });
                        }
                        let subagent_name = arguments.subagent_name.clone();
                        let Some(subagent) = registry.subagent(&subagent_name) else {
                            return Err(ToolError::UserError {
                                tool: tool_name.clone(),
                                message: format!("unknown subagent: {subagent_name}"),
                            });
                        };
                        if let Some(parent_tools) = context.dependency::<SubagentParentTools>() {
                            subagent
                                .tool_inheritance
                                .resolve(&parent_tools.0)
                                .map_err(|error| ToolError::UserError {
                                    tool: tool_name.clone(),
                                    message: error.to_string(),
                                })?;
                        }
                        let supplied_agent_id = arguments
                            .agent_id
                            .clone()
                            .filter(|value| !value.trim().is_empty());
                        let agent_id = supplied_agent_id.clone().unwrap_or_else(|| {
                            format!(
                                "{}-bg-{}",
                                subagent_name,
                                Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
                            )
                        });
                        let in_history = parent_context.subagent_history.contains_key(&agent_id);
                        let known_conversation =
                            monitor.knows_conversation(&agent_id, &subagent_name);
                        if supplied_agent_id.is_some() && !in_history && !known_conversation {
                            return Err(ToolError::UserError {
                                tool: tool_name.clone(),
                                message: "unknown agent_id for this supervisor scope".to_string(),
                            });
                        }
                        let is_resume = in_history || known_conversation;
                        let attempt_id = SubagentAttemptId::new();
                        let linked_task_id = arguments
                            .linked_task_id
                            .as_ref()
                            .map(|value| TaskId::from_string(value.clone()));
                        if let Some(linked_task_id) = linked_task_id.as_ref() {
                            validate_linked_task(
                                &parent_context,
                                linked_task_id,
                                &tool_name,
                            )?;
                        }
                        monitor
                            .accept(BackgroundSubagentAcceptance {
                                attempt_id: attempt_id.clone(),
                                agent_id: agent_id.clone(),
                                subagent_name: subagent_name.clone(),
                                linked_task_id,
                                prompt: arguments.prompt.clone(),
                                parent_session_id: parent_context.session_id.clone(),
                                parent_run_id: parent_context.run_id.clone(),
                                is_resume,
                            })
                            .map_err(|error| background_tool_error(&tool_name, &error))?;
                        let target_agent_id = parent_context.agent_id.as_str().to_string();
                        let background_context = context.clone();
                        tokio::spawn(run_background_delegate(BackgroundDelegateExecution {
                            registry,
                            monitor: monitor.clone(),
                            context_handle,
                            tool_context: background_context,
                            arguments,
                            attempt_id: attempt_id.clone(),
                            agent_id: agent_id.clone(),
                            target_agent_id,
                        }));
                        let status = if is_resume { "continued" } else { "accepted" };
                        Ok(ToolResult::new(serde_json::json!({
                            "status": status,
                            "subagent_name": subagent_name,
                            "attempt_id": attempt_id.as_str(),
                            "agent_id": agent_id,
                            "linked_task_id": monitor
                                .active_tasks()
                                .into_iter()
                                .find(|info| info.attempt_id == attempt_id)
                                .and_then(|info| info.linked_task_id)
                                .map(|task_id| task_id.as_str().to_string()),
                            "message": format!(
                                "{status} delegate: {subagent_name} (agent_id: {agent_id}, attempt_id: {}). Do not manually poll. Use one bounded wait only when blocked; otherwise let the host deliver completion.",
                                attempt_id.as_str()
                            ),
                        })))
                    }
                },
            )
            .with_metadata(explicit_delegation_legacy_metadata())
            .with_tag("delegation"),
        )
    }

    /// Create a blocking delegate instruction block with the available subagent roster.
    #[must_use]
    pub fn delegate_instruction(
        &self,
        parent_tools: Option<&ToolRegistry>,
    ) -> Option<ToolInstruction> {
        let roster = self.roster_instruction(parent_tools)?;
        let content = format!(
            "Delegate bounded subtasks that can return compact results.\n\n\
<delegation-best-practices>\n\
Plan first, then call multiple delegates in the same response for independent work.\n\
Use named specialist subagents when a listed role matches the task.\n\
Ask each delegate to return concise findings, changed files, tests run, and risks.\n\
</delegation-best-practices>\n\n\
{roster}\n\n\
<execution-model>\n\
Delegate calls are blocking: the parent waits for each delegated result before proceeding.\n\
Multiple delegate calls in the same model response run concurrently.\n\
The parent resumes after all delegate calls in that response complete.\n\
Sequential delegate calls across turns run serially.\n\
</execution-model>"
        );
        Some(ToolInstruction::new("delegate", content))
    }

    /// Create an async delegate instruction block with the available subagent roster.
    #[must_use]
    pub fn async_delegate_instruction(
        &self,
        parent_tools: Option<&ToolRegistry>,
    ) -> Option<ToolInstruction> {
        let roster = self.roster_instruction(parent_tools)?;
        let content = format!(
            "In this agent, delegate is asynchronous: it returns a stable agent ID and a per-attempt attempt ID immediately; the final result arrives via message bus.\n\
Use the attempt ID for steer_subagent, cancel_subagent, and wait_subagent. After calling delegate, do not manually poll or loop. If the delegated result is required before you can answer or integrate the work, call wait_subagent once with a bounded timeout. Otherwise finish the current response and let the Starweaver host notify you when the result arrives.\n\
Choose from the available subagents below. Resume a terminal background conversation only when you already have its agent ID; every continuation receives a new attempt ID.\n\n\
{roster}"
        );
        Some(ToolInstruction::new("delegate", content))
    }

    /// Create an explicit `spawn_delegate` instruction block for dual blocking/async mode.
    #[must_use]
    pub fn spawn_delegate_instruction(&self) -> ToolInstruction {
        ToolInstruction::new(
            SPAWN_DELEGATE_TOOL_NAME,
            "Use this to run a subagent asynchronously when immediate results are not required.\n\
Choose the same available subagents listed for delegate.\n\
The call returns right away with a stable agent ID and a per-attempt attempt ID; use the attempt ID for steer_subagent, cancel_subagent, and wait_subagent. Do not manually poll or loop for the result.\n\
If the delegated result is required before you can answer or integrate work, call wait_subagent once with a bounded timeout.\n\
If no other immediate work remains after spawning, finish your current response; the Starweaver host will automatically notify you when the result arrives via message bus.\n\
Resume a terminal background conversation only when you already have its agent ID; every continuation receives a new attempt ID.",
        )
    }

    fn roster_instruction(&self, parent_tools: Option<&ToolRegistry>) -> Option<String> {
        let mut lines = vec!["Available subagents:".to_string()];
        for subagent in &self.subagents {
            if !subagent_available_for_parent(subagent, parent_tools) {
                continue;
            }
            lines.push(format!(
                "<subagent name=\"{}\">",
                escape_xml_attribute(&subagent.name)
            ));
            lines.push(escape_xml_text(
                subagent
                    .description
                    .as_deref()
                    .unwrap_or("Registered subagent"),
            ));
            lines.push("</subagent>\n".to_string());
        }
        (lines.len() > 1).then(|| lines.join("\n").trim_end().to_string())
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
        Box::pin(self.delegate_task_with_stream_sink(name, task, parent_context, None, None)).await
    }

    #[allow(clippy::too_many_lines)]
    async fn delegate_task_with_stream_sink(
        &self,
        name: &str,
        task: SubagentTask,
        parent_context: &mut AgentContext,
        stream_sink: Option<Arc<AgentStreamSink>>,
        background_control: Option<BackgroundSubagentChildControl>,
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
        let child_agent_id = task
            .metadata
            .get("agent_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map_or_else(|| format!("{}-{}", name, task.id.as_str()), str::to_string);
        parent_context.publish_event(starweaver_context::AgentEvent::new(
            "subagent_started",
            serde_json::to_value(
                SubagentLifecycleEvent::new(SubagentLifecycleKind::Started, name, task.id.clone())
                    .with_metadata(serde_json::Value::Object(subagent_base_metadata(
                        &task,
                        Some(&child_agent_id),
                    ))),
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
        let mut child_context = parent_context.subagent_context_with_agent_id(name, child_agent_id);
        child_context.parent_task_id = Some(task.id.clone());
        child_context.metadata.insert(
            "parent_task_id".to_string(),
            serde_json::json!(task.id.as_str()),
        );
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
        if let Some(control) = background_control {
            child_agent = child_agent
                .with_cancellation_token(control.cancellation)
                .with_capability(Arc::new(BackgroundChildControlCapability::new(
                    control.pending_messages,
                )));
        }
        if let Some(stream_sink) = stream_sink.clone() {
            child_agent = child_agent.with_stream_observer(Arc::new(SubagentStreamForwarder::new(
                stream_sink,
                child_context.agent_id.clone(),
                name,
                task.id.clone(),
            )));
        }
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
        let subagent_started_at = std::time::Instant::now();
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
                metadata.insert(
                    "duration_seconds".to_string(),
                    serde_json::json!(subagent_started_at.elapsed().as_secs_f64()),
                );
                metadata.insert(
                    "request_count".to_string(),
                    serde_json::json!(child_context.usage.requests),
                );
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
                        "hook": "after_subagent_run",
                        "duration_seconds": subagent_started_at.elapsed().as_secs_f64(),
                        "request_count": child_context.usage.requests,
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
        let mut completion_metadata =
            subagent_base_metadata(&task, Some(child_context.agent_id.as_str()));
        completion_metadata.insert(
            "duration_seconds".to_string(),
            serde_json::json!(subagent_started_at.elapsed().as_secs_f64()),
        );
        completion_metadata.insert(
            "request_count".to_string(),
            serde_json::json!(result.state.usage.requests),
        );
        completion_metadata.insert(
            "result_preview".to_string(),
            serde_json::json!(compact_preview(&result.output, 240)),
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
                .with_metadata(serde_json::Value::Object(completion_metadata)),
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

struct BackgroundDelegateExecution {
    registry: Arc<SubagentRegistry>,
    monitor: Arc<BackgroundSubagentMonitor>,
    context_handle: Arc<AgentContextHandle>,
    tool_context: ToolContext,
    arguments: DelegateArgs,
    attempt_id: SubagentAttemptId,
    agent_id: String,
    target_agent_id: String,
}

async fn run_background_delegate(execution: BackgroundDelegateExecution) {
    let BackgroundDelegateExecution {
        registry,
        monitor,
        context_handle,
        tool_context,
        arguments,
        attempt_id,
        agent_id,
        target_agent_id,
    } = execution;
    monitor.transition(&attempt_id, BackgroundSubagentExecutionStatus::Starting);
    let cancellation = monitor
        .child_control(&attempt_id)
        .map(|control| control.cancellation);
    let worker = tokio::spawn(run_background_delegate_inner(
        registry,
        monitor.clone(),
        context_handle.clone(),
        tool_context,
        arguments.clone(),
        attempt_id.clone(),
        agent_id,
    ));
    monitor.attach_abort_handle(&attempt_id, worker.abort_handle());

    let (status, content, error, child_run_id) = match worker.await {
        Ok(Ok((output, child_run_id))) => (
            BackgroundSubagentExecutionStatus::Completed,
            Some(output),
            None,
            child_run_id,
        ),
        Ok(Err(error)) => {
            let cancelled = cancellation
                .as_ref()
                .is_some_and(starweaver_core::CancellationToken::is_cancelled);
            (
                if cancelled {
                    BackgroundSubagentExecutionStatus::Cancelled
                } else {
                    BackgroundSubagentExecutionStatus::Failed
                },
                None,
                Some(error.to_string()),
                None,
            )
        }
        Err(join_error) => (
            if join_error.is_cancelled() {
                BackgroundSubagentExecutionStatus::Cancelled
            } else {
                BackgroundSubagentExecutionStatus::Failed
            },
            None,
            Some(if join_error.is_cancelled() {
                "background subagent task aborted after cancellation deadline".to_string()
            } else {
                "background subagent task terminated unexpectedly".to_string()
            }),
            None,
        ),
    };
    monitor.set_child_run_id(&attempt_id, child_run_id);
    let Some(result) = monitor.record_terminal(&attempt_id, status, content, error) else {
        return;
    };
    let message = background_result_message(&monitor, &result, &target_agent_id);
    deliver_background_result(
        &monitor,
        &context_handle,
        &attempt_id,
        &target_agent_id,
        message,
    );
    monitor.notify_completion(&attempt_id);
}

fn background_result_message(
    monitor: &BackgroundSubagentMonitor,
    result: &BackgroundSubagentTaskResult,
    target_agent_id: &str,
) -> BusMessage {
    let message_text = match (&result.content, &result.error) {
        (Some(output), _) => output.clone(),
        (_, Some(error)) => format!(
            "Background delegate '{}' (agent_id: {}, attempt_id: {}) {}: {error}",
            result.subagent_name,
            result.agent_id,
            result.attempt_id.as_str(),
            result.status.as_str(),
        ),
        _ => format!(
            "Background delegate '{}' (agent_id: {}, attempt_id: {}) {}",
            result.subagent_name,
            result.agent_id,
            result.attempt_id.as_str(),
            result.status.as_str(),
        ),
    };
    BusMessage::text(message_text, result.agent_id.clone())
        .with_id(monitor.get_task_result_message_id(&result.attempt_id))
        .with_target(target_agent_id)
}

fn deliver_background_result(
    monitor: &BackgroundSubagentMonitor,
    context_handle: &AgentContextHandle,
    attempt_id: &SubagentAttemptId,
    target_agent_id: &str,
    message: BusMessage,
) {
    if !monitor.is_waiting(attempt_id)
        && monitor.direct_delivery_allowed(attempt_id)
        && context_handle
            .snapshot()
            .messages
            .is_subscribed(target_agent_id)
    {
        let claim_id = format!("active-turn:{}", attempt_id.as_str());
        let claim = BackgroundSubagentDeliveryClaim {
            claim_id: claim_id.clone(),
            continuation_run_id: None,
            deadline: Utc::now() + chrono::Duration::seconds(60),
        };
        if monitor.claim_delivery(attempt_id, claim).is_ok() {
            context_handle.update(|context| {
                context.send_message(message);
            });
            let _ = monitor.acknowledge_delivery(attempt_id, &claim_id);
        }
    } else {
        monitor.enqueue_message(attempt_id.clone(), message);
    }
}

async fn run_background_delegate_inner(
    registry: Arc<SubagentRegistry>,
    monitor: Arc<BackgroundSubagentMonitor>,
    context_handle: Arc<AgentContextHandle>,
    tool_context: ToolContext,
    arguments: DelegateArgs,
    attempt_id: SubagentAttemptId,
    agent_id: String,
) -> Result<(String, Option<starweaver_core::RunId>), AgentError> {
    let mut parent_context = context_handle.snapshot();
    let base_usage = parent_context.usage.clone();
    let base_usage_snapshot_keys = parent_context
        .usage_snapshot_entries
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let base_event_count = parent_context.events.events().len();
    parent_context.trace_context = tool_context.trace_context.clone();
    if let Some(trace_recorder) = tool_context.dependency::<TraceRecorderHandle>() {
        parent_context
            .dependencies
            .insert(trace_recorder.as_ref().clone());
    }
    if let Some(parent_tools) = tool_context.dependency::<SubagentParentTools>() {
        parent_context
            .dependencies
            .insert(parent_tools.as_ref().clone());
    }
    let mut metadata = serde_json::json!({
        "agent_id": agent_id,
        "attempt_id": attempt_id.as_str(),
        "background": true,
    });
    if let Some(linked_task_id) = arguments.linked_task_id.as_ref() {
        metadata["linked_task_id"] = serde_json::json!(linked_task_id);
    }
    let stream_sink = tool_context.dependency::<AgentStreamSink>();
    let task = SubagentTask::new(arguments.prompt).with_metadata(metadata);
    let control = monitor
        .child_control(&attempt_id)
        .ok_or_else(|| AgentError::Capability("background attempt control was lost".to_string()))?;
    monitor.transition(&attempt_id, BackgroundSubagentExecutionStatus::Running);
    let result = Box::pin(registry.delegate_task_with_stream_sink(
        &arguments.subagent_name,
        task,
        &mut parent_context,
        stream_sink,
        Some(control),
    ))
    .await?;
    let child_run_id = Some(result.result.state.run_id.clone());
    let delta = BackgroundSubagentContextDelta::from_context(
        &parent_context,
        &base_usage,
        &base_usage_snapshot_keys,
        base_event_count,
        &agent_id,
    );
    monitor.merge_or_stage_context_delta(&attempt_id, &context_handle, delta);
    Ok((result.output().to_string(), child_run_id))
}

const MAX_WAIT_SUBAGENT_TIMEOUT_SECONDS: f64 = 300.0;

fn normalize_wait_subagent_timeout(timeout_seconds: f64) -> Duration {
    let timeout_seconds = if timeout_seconds.is_finite() {
        timeout_seconds.clamp(0.0, MAX_WAIT_SUBAGENT_TIMEOUT_SECONDS)
    } else {
        MAX_WAIT_SUBAGENT_TIMEOUT_SECONDS
    };
    Duration::from_secs_f64(timeout_seconds)
}

async fn wait_for_one_background_subagent(
    monitor: &BackgroundSubagentMonitor,
    context_handle: &AgentContextHandle,
    attempt_id: &SubagentAttemptId,
    timeout: Duration,
    target: &str,
) -> serde_json::Value {
    let known_ids = monitor.known_task_ids();
    if !known_ids.iter().any(|known_id| known_id == attempt_id) {
        return serde_json::json!({
            "status": "not_found",
            "attempt_id": attempt_id.as_str(),
            "timed_out": false,
            "known_attempt_ids": known_ids
                .iter()
                .map(SubagentAttemptId::as_str)
                .collect::<Vec<_>>(),
        });
    }

    monitor.begin_wait(attempt_id);
    let result = monitor.wait_for_attempt(attempt_id, timeout).await;
    monitor.end_wait(attempt_id);

    let Some(result) = result else {
        let active = monitor
            .active_tasks()
            .into_iter()
            .find(|info| &info.attempt_id == attempt_id);
        return serde_json::json!({
            "status": "running",
            "attempt_id": attempt_id.as_str(),
            "agent_id": active.as_ref().map(|info| info.agent_id.as_str()),
            "timed_out": true,
            "message": "Subagent is still running.",
        });
    };
    consume_background_result(monitor, context_handle, attempt_id, target);
    format_background_result(&result)
}

async fn wait_for_all_background_subagents(
    monitor: &BackgroundSubagentMonitor,
    context_handle: &AgentContextHandle,
    timeout: Duration,
    target: &str,
) -> serde_json::Value {
    let attempt_ids = monitor.known_task_ids();
    if attempt_ids.is_empty() {
        return serde_json::json!({
            "status": "empty",
            "timed_out": false,
            "results": [],
        });
    }

    for attempt_id in &attempt_ids {
        monitor.begin_wait(attempt_id);
    }
    let results_by_id = monitor.wait_for_attempts(&attempt_ids, timeout).await;
    for attempt_id in &attempt_ids {
        monitor.end_wait(attempt_id);
    }

    let active = monitor
        .active_tasks()
        .into_iter()
        .map(|info| (info.attempt_id.clone(), info))
        .collect::<BTreeMap<_, _>>();
    let mut timed_out = false;
    let mut formatted_results = Vec::new();
    for attempt_id in &attempt_ids {
        if let Some(result) = results_by_id.get(attempt_id).and_then(Clone::clone) {
            consume_background_result(monitor, context_handle, attempt_id, target);
            formatted_results.push(format_background_result(&result));
        } else {
            timed_out = true;
            formatted_results.push(serde_json::json!({
                "status": "running",
                "attempt_id": attempt_id.as_str(),
                "agent_id": active.get(attempt_id).map(|info| info.agent_id.as_str()),
                "timed_out": true,
                "message": "Subagent is still running.",
            }));
        }
    }

    let has_terminal = formatted_results
        .iter()
        .any(|item| item.get("status").and_then(serde_json::Value::as_str) != Some("running"));
    let status = if timed_out && has_terminal {
        "partial"
    } else if timed_out {
        "running"
    } else {
        "completed"
    };

    serde_json::json!({
        "status": status,
        "timed_out": timed_out,
        "results": formatted_results,
    })
}

fn consume_background_result(
    monitor: &BackgroundSubagentMonitor,
    context_handle: &AgentContextHandle,
    attempt_id: &SubagentAttemptId,
    target: &str,
) {
    let claim_id = format!("wait:{}", attempt_id.as_str());
    let claim = BackgroundSubagentDeliveryClaim {
        claim_id: claim_id.clone(),
        continuation_run_id: None,
        deadline: Utc::now() + chrono::Duration::seconds(60),
    };
    match monitor.claim_delivery(attempt_id, claim) {
        Ok(_) => {
            let _ = monitor.acknowledge_delivery(attempt_id, &claim_id);
        }
        Err(BackgroundSubagentError::Delivered) => {}
        Err(_) => return,
    }
    let message_id = monitor.get_task_result_message_id(attempt_id);
    context_handle.update(|context| {
        let mut message_ids = BTreeSet::new();
        message_ids.insert(message_id);
        context
            .messages
            .mark_consumed(target.to_string(), &message_ids);
    });
}

fn format_background_result(result: &BackgroundSubagentTaskResult) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "status": result.status.as_str(),
        "attempt_id": result.attempt_id.as_str(),
        "agent_id": result.agent_id,
        "subagent_name": result.subagent_name,
        "linked_task_id": result.linked_task_id.as_ref().map(TaskId::as_str),
        "parent_session_id": result.parent_session_id.as_ref().map(starweaver_core::SessionId::as_str),
        "parent_run_id": result.parent_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "child_run_id": result.child_run_id.as_ref().map(starweaver_core::RunId::as_str),
        "delivery_status": result.delivery_status,
        "retention_status": result.retention_status,
        "failure_category": result.failure_category,
        "cancellation_reason": result.cancellation_reason,
        "timed_out": false,
        "completed_at": result.completed_at.to_rfc3339(),
    });
    if let Some(content) = &result.content {
        payload["result"] = serde_json::json!(content);
    }
    if let Some(error) = &result.error {
        payload["error"] = serde_json::json!(error);
    }
    payload
}

struct BackgroundChildControlCapability {
    pending_messages: Arc<tokio::sync::Mutex<VecDeque<BusMessage>>>,
}

impl BackgroundChildControlCapability {
    const fn new(pending_messages: Arc<tokio::sync::Mutex<VecDeque<BusMessage>>>) -> Self {
        Self { pending_messages }
    }

    async fn drain(&self, context: &mut AgentContext) {
        let mut pending = self.pending_messages.lock().await;
        while let Some(message) = pending.pop_front() {
            context.send_message(message);
        }
    }
}

#[async_trait::async_trait]
impl AgentCapability for BackgroundChildControlCapability {
    fn spec(&self) -> CapabilitySpec {
        CapabilitySpec::new("starweaver.subagent.child_control")
    }

    async fn prepare_run_input_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        input: starweaver_runtime::AgentInput,
    ) -> CapabilityResult<starweaver_runtime::AgentInput> {
        self.drain(context).await;
        Ok(input)
    }

    async fn prepare_model_messages_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        messages: Vec<starweaver_model::ModelMessage>,
    ) -> CapabilityResult<Vec<starweaver_model::ModelMessage>> {
        self.drain(context).await;
        Ok(messages)
    }

    async fn after_output_validation_with_context(
        &self,
        _state: &mut AgentRunState,
        context: &mut AgentContext,
        _output: &str,
    ) -> CapabilityResult<()> {
        self.drain(context).await;
        Ok(())
    }
}

fn is_main_agent_context(context: &AgentContext) -> bool {
    context.parent_run_id.is_none()
        && !context.metadata.contains_key("parent_agent_id")
        && context.agent_id.as_str() == "main"
}

fn ensure_main_agent_tool(context: &ToolContext, tool_name: &str) -> Result<(), ToolError> {
    let context_handle =
        context
            .dependency::<AgentContextHandle>()
            .ok_or_else(|| ToolError::UserError {
                tool: tool_name.to_string(),
                message: "missing AgentContextHandle dependency".to_string(),
            })?;
    if !is_main_agent_context(&context_handle.snapshot()) {
        return Err(ToolError::UserError {
            tool: tool_name.to_string(),
            message: format!("{tool_name} is only available to the owning main agent"),
        });
    }
    Ok(())
}

fn background_tool_error(tool_name: &str, error: &BackgroundSubagentError) -> ToolError {
    ToolError::UserError {
        tool: tool_name.to_string(),
        message: error.to_string(),
    }
}

fn validate_linked_task(
    context: &AgentContext,
    linked_task_id: &TaskId,
    tool_name: &str,
) -> Result<(), ToolError> {
    let Some(task) = context.tools.tasks.tasks.get(linked_task_id.as_str()) else {
        return Err(ToolError::UserError {
            tool: tool_name.to_string(),
            message: "linked_task_id is not present in the parent task scope".to_string(),
        });
    };
    if task.status.is_completed() {
        return Err(ToolError::UserError {
            tool: tool_name.to_string(),
            message: "linked task is already completed".to_string(),
        });
    }
    if task.is_blocked() {
        return Err(ToolError::UserError {
            tool: tool_name.to_string(),
            message: "linked task is blocked".to_string(),
        });
    }
    if task
        .owner
        .as_deref()
        .is_some_and(|owner| owner != context.agent_id.as_str() && owner != "main")
    {
        return Err(ToolError::UserError {
            tool: tool_name.to_string(),
            message: "linked task is owned by another worker".to_string(),
        });
    }
    Ok(())
}

/// Parent tool registry dependency used to resolve subagent inherited tools.
#[derive(Clone)]
pub struct SubagentParentTools(pub ToolRegistry);

struct SubagentStreamForwarder {
    stream_sink: Arc<AgentStreamSink>,
    child_agent_id: AgentId,
    subagent_name: String,
    task_id: TaskId,
}

impl SubagentStreamForwarder {
    fn new(
        stream_sink: Arc<AgentStreamSink>,
        child_agent_id: AgentId,
        subagent_name: impl Into<String>,
        task_id: TaskId,
    ) -> Self {
        Self {
            stream_sink,
            child_agent_id,
            subagent_name: subagent_name.into(),
            task_id,
        }
    }
}

#[async_trait::async_trait]
impl AgentCapability for SubagentStreamForwarder {
    async fn on_stream_event_with_context(
        &self,
        _state: &AgentRunState,
        context: &AgentContext,
        record: &AgentStreamRecord,
    ) -> CapabilityResult<()> {
        self.stream_sink
            .push(record.clone().with_source(AgentStreamSource::subagent(
                self.child_agent_id.clone(),
                self.subagent_name.clone(),
                self.task_id.clone(),
                context.run_id.clone(),
                context.parent_run_id.clone(),
                record.sequence,
            )));
        Ok(())
    }
}

fn subagent_base_metadata(
    task: &SubagentTask,
    agent_id: Option<&str>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut metadata = task
        .metadata
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    if let Some(agent_id) = agent_id.filter(|value| !value.trim().is_empty()) {
        metadata.insert("agent_id".to_string(), serde_json::json!(agent_id));
    }
    metadata.insert(
        "prompt_preview".to_string(),
        serde_json::json!(compact_preview(&task.prompt, 240)),
    );
    metadata
}

fn compact_preview(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }
    let keep = max_chars.saturating_sub(1);
    let mut preview = compact.chars().take(keep).collect::<String>();
    preview.push('…');
    preview
}

fn publish_subagent_stream_records(
    parent_context: &mut AgentContext,
    name: &str,
    task_id: &TaskId,
    child_context: &AgentContext,
    records: &[AgentStreamRecord],
    stream_sink: Option<&AgentStreamSink>,
) {
    if stream_sink.is_some() {
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

fn subagent_available_for_parent(
    subagent: &SubagentConfig,
    parent_tools: Option<&ToolRegistry>,
) -> bool {
    parent_tools.is_none_or(|tools| subagent.tool_inheritance.resolve(tools).is_ok())
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
