use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use starweaver_context::{AgentContext, AgentContextHandle, AgentEvent, BusMessage};
use starweaver_core::{
    AgentId, Metadata, SubagentLifecycleEvent, SubagentLifecycleKind, TaskId, escape_xml_attribute,
    escape_xml_text,
};
use starweaver_runtime::{
    AgentCapability, AgentError, AgentResult, AgentRunState, AgentStreamRecord, AgentStreamSink,
    AgentStreamSource, CapabilityBundle, CapabilityResult, CapabilitySpec, TraceRecorderHandle,
};
use starweaver_tools::{
    DynTool, EmptyToolArgs, ToolContext, ToolError, ToolInstruction, ToolRegistry, ToolResult,
    typed_json_tool,
};
use uuid::Uuid;

use crate::bundles::attach_environment;

use super::{
    SubagentConfig, SubagentExecutionMetadata, SubagentExecutionOutcome, SubagentResult,
    SubagentTask, SubagentToolInheritanceError,
};

const SUBAGENT_STACK_KEY: &str = "starweaver.subagent_stack";

/// Hidden delegate backend tool used by async delegation wrappers.
pub const DELEGATE_BACKEND_TOOL_NAME: &str = "__delegate_backend";

/// Tool name for explicit background delegation when blocking delegate remains visible.
pub const SPAWN_DELEGATE_TOOL_NAME: &str = "spawn_delegate";

const BACKGROUND_SUBAGENT_CAPABILITY_ID: &str = "starweaver.subagent.background";

/// Model-visible subagent delegation topology.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SubagentDelegationMode {
    /// Expose `delegate` as a blocking tool.
    #[default]
    Blocking,
    /// Expose `delegate` as an asynchronous background tool backed by hidden `__delegate_backend`.
    Async,
    /// Expose blocking `delegate` plus explicit `spawn_delegate`.
    BlockingAndAsync,
}

impl SubagentDelegationMode {
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

/// Snapshot of one active background subagent task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundSubagentTaskInfo {
    /// Stable background subagent id.
    pub agent_id: String,
    /// Registered subagent name.
    pub subagent_name: String,
    /// Prompt sent to the background subagent.
    pub prompt: String,
    /// Whether this task resumes an existing subagent conversation.
    pub is_resume: bool,
}

#[derive(Default)]
struct BackgroundSubagentState {
    active_tasks: BTreeMap<String, BackgroundSubagentTaskInfo>,
    pending_messages: Vec<BusMessage>,
}

/// Shared monitor for detached subagent runs and pending result redelivery.
#[derive(Default)]
pub struct BackgroundSubagentMonitor {
    state: Mutex<BackgroundSubagentState>,
}

impl BackgroundSubagentMonitor {
    /// Create an empty background subagent monitor.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn register_task(
        &self,
        agent_id: String,
        subagent_name: String,
        prompt: String,
        is_resume: bool,
    ) {
        let info = BackgroundSubagentTaskInfo {
            agent_id: agent_id.clone(),
            subagent_name,
            prompt,
            is_resume,
        };
        match self.state.lock() {
            Ok(mut state) => {
                state.active_tasks.insert(agent_id, info);
            }
            Err(error) => {
                let mut state = error.into_inner();
                state.active_tasks.insert(agent_id, info);
            }
        }
    }

    fn complete_task(&self, agent_id: &str) {
        match self.state.lock() {
            Ok(mut state) => {
                state.active_tasks.remove(agent_id);
            }
            Err(error) => {
                let mut state = error.into_inner();
                state.active_tasks.remove(agent_id);
            }
        }
    }

    /// Return active background subagent tasks.
    #[must_use]
    pub fn active_tasks(&self) -> Vec<BackgroundSubagentTaskInfo> {
        match self.state.lock() {
            Ok(state) => state.active_tasks.values().cloned().collect(),
            Err(error) => error.into_inner().active_tasks.values().cloned().collect(),
        }
    }

    /// Return whether any background task is active.
    #[must_use]
    pub fn has_active_tasks(&self) -> bool {
        match self.state.lock() {
            Ok(state) => !state.active_tasks.is_empty(),
            Err(error) => !error.into_inner().active_tasks.is_empty(),
        }
    }

    fn enqueue_message(&self, message: BusMessage) {
        match self.state.lock() {
            Ok(mut state) => state.pending_messages.push(message),
            Err(error) => {
                let mut state = error.into_inner();
                state.pending_messages.push(message);
            }
        }
    }

    /// Return whether pending completion messages are waiting for redelivery.
    #[must_use]
    pub fn has_pending_messages(&self) -> bool {
        match self.state.lock() {
            Ok(state) => !state.pending_messages.is_empty(),
            Err(error) => !error.into_inner().pending_messages.is_empty(),
        }
    }

    fn drain_pending_messages(&self) -> Vec<BusMessage> {
        match self.state.lock() {
            Ok(mut state) => std::mem::take(&mut state.pending_messages),
            Err(error) => {
                let mut state = error.into_inner();
                std::mem::take(&mut state.pending_messages)
            }
        }
    }
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
            "Delegate task to a registered SDK subagent asynchronously. Result delivered via message bus.",
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
            "Spawn a registered SDK subagent in the background. Result delivered via message bus.",
            monitor,
        )
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
        self.delegate_tool_named_with_visibility(tool_name, true)
    }

    fn delegate_tool_named_with_visibility(
        self: &Arc<Self>,
        tool_name: impl Into<String>,
        visible: bool,
    ) -> DynTool {
        let registry = self.clone();
        let tool_name = tool_name.into();
        let tool = typed_json_tool::<DelegateArgs, _, _>(
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
        )
        .with_tag("delegation");
        if visible {
            Arc::new(tool)
        } else {
            Arc::new(tool.with_prepare_definition(|_, _| None))
        }
    }

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
                                ToolError::Execution {
                                    tool: tool_name.clone(),
                                    message: "missing AgentContextHandle dependency".to_string(),
                                }
                            })?;
                        let parent_context = context_handle.snapshot();
                        if parent_context.parent_run_id.is_some()
                            || parent_context.metadata.contains_key("parent_agent_id")
                        {
                            return Err(ToolError::Execution {
                                tool: tool_name.clone(),
                                message: "background subagent delegation is only available to the main agent"
                                    .to_string(),
                            });
                        }
                        let subagent_name = arguments.subagent_name.clone();
                        let agent_id = arguments
                            .agent_id
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| {
                                format!(
                                    "{}-bg-{}",
                                    subagent_name,
                                    Uuid::new_v4().simple().to_string().chars().take(8).collect::<String>()
                                )
                            });
                        let is_resume = parent_context.subagent_history.contains_key(&agent_id);
                        monitor.register_task(
                            agent_id.clone(),
                            subagent_name.clone(),
                            arguments.prompt.clone(),
                            is_resume,
                        );
                        let target_agent_id = parent_context.agent_id.as_str().to_string();
                        let background_context = context.clone();
                        tokio::spawn(run_background_delegate(
                            registry,
                            monitor.clone(),
                            context_handle,
                            background_context,
                            arguments,
                            agent_id.clone(),
                            target_agent_id,
                        ));
                        let action = if is_resume { "resumed" } else { "spawned" };
                        Ok(ToolResult::new(serde_json::json!({
                            "status": action,
                            "subagent_name": subagent_name,
                            "agent_id": agent_id,
                            "message": format!(
                                "{action} delegate: {subagent_name} (id: {agent_id}). Result will be delivered via message bus when complete."
                            ),
                        })))
                    }
                },
            )
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
            "Use the delegate tool for bounded subtasks that can return compact results.\n\n\
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
            "In this agent, delegate is asynchronous: it returns an agent ID immediately; the final result arrives via message bus.\n\
Use subagent_name from the available subagents below. Pass agent_id to resume a previous background subagent.\n\n\
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
Use the same subagent_name values listed for delegate.\n\
The call returns right away with an agent ID; the final result is delivered via message bus.\n\
Pass agent_id to resume a previous background subagent.",
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

async fn run_background_delegate(
    registry: Arc<SubagentRegistry>,
    monitor: Arc<BackgroundSubagentMonitor>,
    context_handle: Arc<AgentContextHandle>,
    tool_context: ToolContext,
    mut arguments: DelegateArgs,
    agent_id: String,
    target_agent_id: String,
) {
    let message = match Box::pin(run_background_delegate_inner(
        registry,
        &context_handle,
        &tool_context,
        &mut arguments,
        &agent_id,
    ))
    .await
    {
        Ok(output) => {
            BusMessage::text(output, agent_id.clone()).with_target(target_agent_id.as_str())
        }
        Err(error) => BusMessage::text(
            format!(
                "Spawned delegate '{}' (id: {agent_id}) failed: {error}",
                arguments.subagent_name
            ),
            agent_id.clone(),
        )
        .with_target(target_agent_id.as_str()),
    };
    if context_handle
        .snapshot()
        .messages
        .is_subscribed(&target_agent_id)
    {
        context_handle.update(|context| {
            context.send_message(message.clone());
        });
    } else {
        monitor.enqueue_message(message);
    }
    monitor.complete_task(&agent_id);
}

async fn run_background_delegate_inner(
    registry: Arc<SubagentRegistry>,
    context_handle: &AgentContextHandle,
    tool_context: &ToolContext,
    arguments: &mut DelegateArgs,
    agent_id: &str,
) -> Result<String, AgentError> {
    let mut parent_context = context_handle.snapshot();
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
    let mut metadata = arguments
        .metadata
        .take()
        .unwrap_or_else(|| serde_json::json!({}));
    metadata["agent_id"] = serde_json::json!(agent_id);
    metadata["background"] = serde_json::json!(true);
    let stream_sink = tool_context.dependency::<AgentStreamSink>();
    let task = SubagentTask::new(arguments.prompt.clone()).with_metadata(metadata);
    let result = Box::pin(registry.delegate_task_with_stream_sink(
        &arguments.subagent_name,
        task,
        &mut parent_context,
        stream_sink,
    ))
    .await?;
    context_handle.update(|context| merge_background_subagent_context(context, &parent_context));
    Ok(result.output().to_string())
}

fn merge_background_subagent_context(target: &mut AgentContext, source: &AgentContext) {
    target.usage = source.usage.clone();
    target
        .usage_snapshot_entries
        .clone_from(&source.usage_snapshot_entries);
    for (agent_id, info) in &source.agent_registry {
        target.agent_registry.insert(agent_id.clone(), info.clone());
    }
    for (agent_id, history) in &source.subagent_history {
        target
            .subagent_history
            .insert(agent_id.clone(), history.clone());
    }
    for event in source.events.events() {
        if !target.events.events().contains(event) {
            target.events.publish(event.clone());
        }
    }
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
