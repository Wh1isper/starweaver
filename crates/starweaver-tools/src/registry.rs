//! Tool registry and execution dispatch.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use serde::{Deserialize, Serialize};
use starweaver_context::AgentContext;
use starweaver_model::{ToolCallPart, ToolDefinition, ToolReturnPart};

use crate::{
    DynTool, DynToolExecutionHook, DynToolset, ToolContext, ToolError, ToolExecutionHooks,
    ToolExecutionOutcome, ToolInstruction, ToolResult, ToolsetLifecycleError,
    ToolsetLifecycleReport, ToolsetPreparation, error_return,
};

/// Default retry budget for unexpected/internal tool execution failures.
pub const DEFAULT_TOOL_MAX_RETRIES: usize = 3;

fn success_return(call: &ToolCallPart, result: ToolResult) -> ToolReturnPart {
    let model_return_content = result
        .model_content
        .clone()
        .unwrap_or_else(|| result.content.clone());
    ToolReturnPart {
        tool_call_id: call.id.clone(),
        name: call.name.clone(),
        content: model_return_content,
        is_error: false,
        metadata: result.metadata,
        app_value: result.app_value,
        user_content: result.user_content,
        private_metadata: result.private_metadata,
    }
}

fn retry_error_return(
    call: &ToolCallPart,
    error: &ToolError,
    attempt: usize,
    max_retries: usize,
) -> ToolReturnPart {
    let mut returned = error_return(call, error);
    if error.unexpected() {
        returned
            .metadata
            .insert("tool_retry".to_string(), serde_json::json!(attempt));
        returned
            .metadata
            .insert("max_retries".to_string(), serde_json::json!(max_retries));
        if let Some(content) = returned.content.as_object_mut() {
            content.insert("tool_retry".to_string(), serde_json::json!(attempt));
            content.insert("max_retries".to_string(), serde_json::json!(max_retries));
        }
    }
    returned
}

/// Report describing which tools are visible for a specific agent context.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolAvailabilityReport {
    /// Tool names exposed to the model.
    pub available: Vec<String>,
    /// Tool names skipped by context-aware availability predicates.
    pub unavailable: Vec<String>,
}

impl ToolAvailabilityReport {
    /// Return whether no tools were skipped.
    #[must_use]
    pub const fn is_all_available(&self) -> bool {
        self.unavailable.is_empty()
    }

    /// Return whether no tools are exposed.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.available.is_empty() && self.unavailable.is_empty()
    }
}

/// Tool registry used by agent runs.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, DynTool>,
    toolset_max_retries: BTreeMap<String, usize>,
    toolset_timeouts_ms: BTreeMap<String, u64>,
    instructions: BTreeMap<String, ToolInstruction>,
    execution_hooks: ToolExecutionHooks,
    max_retries: Option<usize>,
    timeout_ms: Option<u64>,
}

impl ToolRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool.
    #[must_use]
    pub fn with_tool(mut self, tool: DynTool) -> Self {
        self.insert(tool);
        self
    }

    /// Insert or replace a tool.
    pub fn insert(&mut self, tool: DynTool) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Set an agent-level retry default for tools that do not override it.
    #[must_use]
    pub const fn with_max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = Some(max_retries);
        self
    }

    /// Update the agent-level retry default.
    pub const fn set_max_retries(&mut self, max_retries: usize) {
        self.max_retries = Some(max_retries);
    }

    /// Set an agent-level execution timeout default for tools that do not override it.
    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    /// Update the agent-level execution timeout default.
    pub const fn set_timeout_ms(&mut self, timeout_ms: u64) {
        self.timeout_ms = Some(timeout_ms);
    }

    /// Add a global execution hook.
    #[must_use]
    pub fn with_global_execution_hook(mut self, hook: DynToolExecutionHook) -> Self {
        self.insert_global_execution_hook(hook);
        self
    }

    /// Insert a global execution hook.
    pub fn insert_global_execution_hook(&mut self, hook: DynToolExecutionHook) {
        self.execution_hooks.push_global_hook(hook);
    }

    /// Add a tool-specific execution hook.
    #[must_use]
    pub fn with_tool_execution_hook(
        mut self,
        tool: impl Into<String>,
        hook: DynToolExecutionHook,
    ) -> Self {
        self.insert_tool_execution_hook(tool, hook);
        self
    }

    /// Insert a tool-specific execution hook.
    pub fn insert_tool_execution_hook(
        &mut self,
        tool: impl Into<String>,
        hook: DynToolExecutionHook,
    ) {
        self.execution_hooks.push_tool_hook(tool, hook);
    }

    /// Return execution hooks registered on this registry.
    #[must_use]
    pub const fn execution_hooks(&self) -> &ToolExecutionHooks {
        &self.execution_hooks
    }

    /// Add an instruction block, deduplicated by group.
    pub fn insert_instruction(&mut self, instruction: ToolInstruction) {
        self.instructions
            .entry(instruction.group.clone())
            .or_insert(instruction);
    }

    /// Add all tools and instructions from a toolset.
    #[must_use]
    pub fn with_toolset(mut self, toolset: &DynToolset) -> Self {
        self.insert_toolset(toolset);
        self
    }

    /// Insert all tools and instructions from a toolset.
    pub fn insert_toolset(&mut self, toolset: &DynToolset) {
        self.insert_prepared_toolset(toolset, toolset.get_tools(), toolset.get_instructions());
    }

    /// Prepare and insert all tools and instructions from a toolset for a context.
    ///
    /// The lifecycle report is published into the context event bus using the report state's
    /// default event kind. Failed lifecycle operations also publish a failure/unavailable
    /// report before returning the error.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error when context-aware preparation fails or exceeds its
    /// configured timeout.
    pub async fn insert_toolset_with_context(
        &mut self,
        context: &mut AgentContext,
        toolset: &DynToolset,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        self.insert_toolset_with_context_mode(context, toolset, true)
            .await
    }

    /// Refresh and insert all tools and instructions from a toolset without re-entering it.
    ///
    /// This is used when a run advances to a new model step and the runtime needs
    /// context-aware inventory to update without re-running lifecycle enter hooks.
    ///
    /// # Errors
    ///
    /// Returns a lifecycle error when context-aware preparation fails or exceeds its
    /// configured timeout.
    pub async fn refresh_toolset_with_context(
        &mut self,
        context: &mut AgentContext,
        toolset: &DynToolset,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        self.insert_toolset_with_context_mode(context, toolset, false)
            .await
    }

    async fn insert_toolset_with_context_mode(
        &mut self,
        context: &mut AgentContext,
        toolset: &DynToolset,
        enter_before_prepare: bool,
    ) -> Result<ToolsetLifecycleReport, ToolsetLifecycleError> {
        let policy = toolset.lifecycle_policy();
        if enter_before_prepare && policy.enter_before_prepare {
            let enter_result = if let Some(timeout_ms) = policy.initialization_timeout_ms {
                tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    toolset.enter_with_context(context),
                )
                .await
                .map_err(|_| ToolsetLifecycleError::timeout(toolset.name(), timeout_ms))?
            } else {
                toolset.enter_with_context(context).await
            };
            match enter_result {
                Ok(report) => context.publish_event(report.into_event()),
                Err(error) => {
                    let report = error.to_report(toolset.id().map(ToOwned::to_owned));
                    context.publish_event(report.into_event());
                    return Err(error);
                }
            }
        }
        let preparation =
            if let Some(timeout_ms) = policy.read_timeout_ms.or(policy.initialization_timeout_ms) {
                tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    toolset.prepare_with_context(context),
                )
                .await
                .map_err(|_| ToolsetLifecycleError::timeout(toolset.name(), timeout_ms))?
            } else {
                toolset.prepare_with_context(context).await
            };
        match preparation {
            Ok(preparation) => {
                let ToolsetPreparation {
                    tools,
                    instructions,
                    report,
                } = preparation;
                let should_fail = policy.fail_on_unavailable
                    && report.state == crate::ToolsetLifecycleState::Unavailable;
                if let Err(error) = self.validate_prepared_toolset_names(toolset, &tools) {
                    let report = error.to_report(toolset.id().map(ToOwned::to_owned));
                    context.publish_event(report.into_event());
                    return Err(error);
                }
                self.insert_prepared_toolset(toolset, tools, instructions);
                let report_for_event = report.clone();
                context.publish_event(report_for_event.into_event());
                if should_fail {
                    let message = report
                        .message
                        .as_deref()
                        .unwrap_or("toolset unavailable")
                        .to_string();
                    return Err(ToolsetLifecycleError::unavailable(toolset.name(), message));
                }
                Ok(report)
            }
            Err(error) => {
                let report = error.to_report(toolset.id().map(ToOwned::to_owned));
                context.publish_event(report.into_event());
                Err(error)
            }
        }
    }

    fn insert_prepared_toolset(
        &mut self,
        toolset: &DynToolset,
        tools: Vec<DynTool>,
        instructions: Vec<ToolInstruction>,
    ) {
        let max_retries = toolset.max_retries();
        let timeout_ms = toolset.timeout_ms();
        for tool in tools {
            if let Some(max_retries) = max_retries
                && tool.max_retries().is_none()
            {
                self.toolset_max_retries
                    .insert(tool.name().to_string(), max_retries);
            }
            if let Some(timeout_ms) = timeout_ms
                && tool.timeout_ms().is_none()
            {
                self.toolset_timeouts_ms
                    .insert(tool.name().to_string(), timeout_ms);
            }
            self.insert(tool);
        }
        for instruction in instructions {
            self.insert_instruction(instruction);
        }
    }

    fn validate_prepared_toolset_names(
        &self,
        toolset: &DynToolset,
        tools: &[DynTool],
    ) -> Result<(), ToolsetLifecycleError> {
        let mut seen = BTreeSet::new();
        for tool in tools {
            let name = tool.name();
            if !seen.insert(name.to_string()) {
                return Err(ToolsetLifecycleError::failed(
                    toolset.name(),
                    format!("duplicate tool name {name:?} within prepared toolset"),
                ));
            }
            if self.contains(name) {
                return Err(ToolsetLifecycleError::failed(
                    toolset.name(),
                    format!("duplicate tool name {name:?} across prepared toolsets"),
                ));
            }
        }
        Ok(())
    }

    /// Insert all tools and instructions from another registry.
    pub fn insert_registry(&mut self, registry: &Self) {
        if let Some(max_retries) = registry.max_retries {
            self.max_retries = Some(max_retries);
        }
        if let Some(timeout_ms) = registry.timeout_ms {
            self.timeout_ms = Some(timeout_ms);
        }
        for (name, max_retries) in &registry.toolset_max_retries {
            self.toolset_max_retries.insert(name.clone(), *max_retries);
        }
        for (name, timeout_ms) in &registry.toolset_timeouts_ms {
            self.toolset_timeouts_ms.insert(name.clone(), *timeout_ms);
        }
        self.execution_hooks.extend(&registry.execution_hooks);
        for tool in registry.tools.values() {
            self.insert(tool.clone());
        }
        for instruction in registry.instructions.values() {
            self.insert_instruction(instruction.clone());
        }
    }

    /// Return instruction blocks in stable group order.
    #[must_use]
    pub fn instructions(&self) -> Vec<ToolInstruction> {
        self.instructions.values().cloned().collect()
    }

    /// Return rendered instruction text in stable group order.
    #[must_use]
    pub fn get_instructions(&self) -> Vec<String> {
        self.instructions
            .values()
            .map(ToolInstruction::render_xml)
            .collect()
    }

    /// Return all tool definitions sorted by name.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    /// Return tool definitions available for the current agent context, sorted by name.
    #[must_use]
    pub fn definitions_for_context(&self, context: &AgentContext) -> Vec<ToolDefinition> {
        self.definitions_and_availability_for_context(context).0
    }

    /// Return tool definitions and availability diagnostics for the current context.
    #[must_use]
    pub fn definitions_and_availability_for_context(
        &self,
        context: &AgentContext,
    ) -> (Vec<ToolDefinition>, ToolAvailabilityReport) {
        let mut definitions = Vec::new();
        let mut report = ToolAvailabilityReport::default();
        for tool in self.tools.values() {
            if tool.is_available(context) {
                if let Some(definition) = tool.prepare_definition(context, tool.definition()) {
                    report.available.push(tool.name().to_string());
                    definitions.push(definition);
                } else {
                    report.unavailable.push(tool.name().to_string());
                }
            } else {
                report.unavailable.push(tool.name().to_string());
            }
        }
        (definitions, report)
    }

    /// Return context-aware availability diagnostics without model definitions.
    #[must_use]
    pub fn availability_report(&self, context: &AgentContext) -> ToolAvailabilityReport {
        self.tools
            .values()
            .fold(ToolAvailabilityReport::default(), |mut report, tool| {
                if tool.is_available(context)
                    && tool
                        .prepare_definition(context, tool.definition())
                        .is_some()
                {
                    report.available.push(tool.name().to_string());
                } else {
                    report.unavailable.push(tool.name().to_string());
                }
                report
            })
    }

    /// Return whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute one tool call and return a model tool return part.
    pub async fn execute_call(
        &self,
        mut context: ToolContext,
        call: &ToolCallPart,
    ) -> ToolReturnPart {
        context
            .metadata
            .entry("tool_call_id".to_string())
            .or_insert_with(|| serde_json::json!(call.id.clone()));
        context
            .metadata
            .entry("tool_name".to_string())
            .or_insert_with(|| serde_json::json!(call.name.clone()));
        let Some(tool) = self.tools.get(&call.name) else {
            return error_return(call, &ToolError::NotFound(call.name.clone()));
        };
        self.execute_registered_call(tool, context, call).await
    }

    async fn execute_registered_call(
        &self,
        tool: &DynTool,
        mut context: ToolContext,
        call: &ToolCallPart,
    ) -> ToolReturnPart {
        if let Some(error) = call.arguments.invalid_error() {
            return error_return(
                call,
                &ToolError::InvalidArguments {
                    tool: call.name.clone(),
                    message: format!("tool arguments must be valid JSON before execution: {error}"),
                },
            );
        }
        let mut arguments = call.arguments.execution_value();
        if let Err(error) = self
            .execution_hooks
            .run_before(&mut context, call, &mut arguments)
            .await
        {
            return error_return(call, &error);
        }
        let timeout_ms = self.timeout_ms_for(&call.name);
        let max_retries = self.max_retries_for(&call.name);
        let base_retry = context.retry;
        let mut attempt = 0;
        loop {
            let attempt_context = context
                .clone()
                .with_retry_budget(base_retry.saturating_add(attempt), max_retries);
            let result = self
                .execute_tool_attempt(
                    tool,
                    attempt_context.clone(),
                    call,
                    arguments.clone(),
                    timeout_ms,
                )
                .await;
            let mut outcome = ToolExecutionOutcome::from_result(result);
            if let Err(error) = self
                .execution_hooks
                .run_after(&attempt_context, call, &mut outcome)
                .await
            {
                outcome = ToolExecutionOutcome::Error(error);
            }
            match outcome.into_result() {
                Ok(result) => return success_return(call, result),
                Err(error) if error.unexpected() && attempt < max_retries => {
                    attempt = attempt.saturating_add(1);
                }
                Err(error) => return retry_error_return(call, &error, attempt, max_retries),
            }
        }
    }

    async fn execute_tool_attempt(
        &self,
        tool: &DynTool,
        context: ToolContext,
        call: &ToolCallPart,
        arguments: serde_json::Value,
        timeout_ms: Option<u64>,
    ) -> Result<ToolResult, ToolError> {
        let cancellation_token = context.cancellation_token();
        let cancelled = || ToolError::Cancelled {
            tool: call.name.clone(),
            reason: "agent run cancellation requested".to_string(),
        };
        if cancellation_token.is_cancelled() {
            return Err(cancelled());
        }
        if let Some(timeout_ms) = timeout_ms {
            return tokio::select! {
                biased;
                () = cancellation_token.cancelled() => Err(cancelled()),
                result = tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    tool.call(context, arguments),
                ) => result.unwrap_or_else(|_| {
                    Err(ToolError::Timeout {
                        tool: call.name.clone(),
                        timeout_ms,
                    })
                }),
            };
        }
        tokio::select! {
            biased;
            () = cancellation_token.cancelled() => Err(cancelled()),
            result = tool.call(context, arguments) => result,
        }
    }

    /// Return the effective retry limit for a registered tool.
    #[must_use]
    pub fn max_retries_for(&self, name: &str) -> usize {
        self.tools.get(name).map_or_else(
            || self.max_retries.unwrap_or(DEFAULT_TOOL_MAX_RETRIES),
            |tool| {
                tool.max_retries()
                    .or_else(|| self.toolset_max_retries.get(name).copied())
                    .or(self.max_retries)
                    .unwrap_or(DEFAULT_TOOL_MAX_RETRIES)
            },
        )
    }

    /// Return this registry's agent-level retry default.
    #[must_use]
    pub const fn max_retries(&self) -> Option<usize> {
        self.max_retries
    }

    /// Return the effective execution timeout for a registered tool.
    #[must_use]
    pub fn timeout_ms_for(&self, name: &str) -> Option<u64> {
        self.tools.get(name).and_then(|tool| {
            tool.timeout_ms()
                .or_else(|| self.toolset_timeouts_ms.get(name).copied())
                .or(self.timeout_ms)
        })
    }

    /// Return this registry's agent-level execution timeout default.
    #[must_use]
    pub const fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }

    /// Return whether the registered tool requests sequential runtime execution.
    #[must_use]
    pub fn sequential_for(&self, name: &str) -> bool {
        self.tools
            .get(name)
            .and_then(|tool| tool.sequential())
            .unwrap_or(false)
    }

    /// Return whether a tool is registered by name.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Return registered tool names in stable order.
    #[must_use]
    pub fn names(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Return all registered tools in stable name order.
    #[must_use]
    pub fn tools(&self) -> Vec<DynTool> {
        self.tools.values().cloned().collect()
    }

    /// Remove one tool by name.
    pub fn remove(&mut self, name: &str) -> Option<DynTool> {
        self.toolset_max_retries.remove(name);
        self.toolset_timeouts_ms.remove(name);
        self.tools.remove(name)
    }

    /// Return a registry containing a selected subset of tools.
    #[must_use]
    pub fn select(&self, names: impl IntoIterator<Item = impl AsRef<str>>) -> Self {
        let mut selected = Self::new();
        if let Some(max_retries) = self.max_retries {
            selected.max_retries = Some(max_retries);
        }
        if let Some(timeout_ms) = self.timeout_ms {
            selected.timeout_ms = Some(timeout_ms);
        }
        for name in names {
            let name = name.as_ref();
            if let Some(tool) = self.tools.get(name) {
                if let Some(max_retries) = self.toolset_max_retries.get(name) {
                    selected
                        .toolset_max_retries
                        .insert(name.to_string(), *max_retries);
                }
                if let Some(timeout_ms) = self.toolset_timeouts_ms.get(name) {
                    selected
                        .toolset_timeouts_ms
                        .insert(name.to_string(), *timeout_ms);
                }
                selected.insert(tool.clone());
            }
        }
        let selected_names = selected.names();
        selected.execution_hooks = self
            .execution_hooks
            .select_for_tools(selected_names.iter().map(String::as_str));
        selected
    }

    /// Return a registry containing tools whose metadata opts into subagent inheritance.
    #[must_use]
    pub fn auto_inherited(&self) -> Self {
        let names = self
            .tools
            .iter()
            .filter_map(|(name, tool)| {
                tool.metadata()
                    .get("auto_inherit")
                    .and_then(serde_json::Value::as_bool)
                    .filter(|enabled| *enabled)
                    .map(|_| name.clone())
            })
            .collect::<Vec<_>>();
        self.select(names)
    }

    /// Return a registered tool by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<DynTool> {
        self.tools.get(name).cloned()
    }
}
