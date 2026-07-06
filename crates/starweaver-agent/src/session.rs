//! SDK session wrapper for context-backed multi-run applications.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_context::{AgentContext, AgentContextHandle, BusMessage, ResumableState};
use starweaver_core::{Metadata, SessionId, TraceContext};
use starweaver_environment::{DynEnvironmentProvider, EnvironmentError, EnvironmentState};
use starweaver_model::{ModelRequestParameters, ModelSettings, ToolCallPart, ToolReturnPart};
use starweaver_session::{
    DeferredToolRecord, DeferredToolResult, DeferredToolResults,
    ExecutionStatus as SessionExecutionStatus, ToolApprovalDecision, ToolReturnRecordInput,
};
use starweaver_tools::{
    DynTool, DynToolset, ToolApprovalState, ToolContext, ToolError, ToolRegistry,
    ToolUserInputPreprocessResult, error_return,
};
use thiserror::Error;

use crate::streaming::{
    AgentStreamError, AgentStreamHandle, AgentStreamOptions, start_session_stream,
    start_session_stream_with_options, try_start_session_stream,
    try_start_session_stream_with_options,
};
use crate::{EnvironmentHandle, attach_environment};
use starweaver_runtime::{
    Agent as RuntimeAgent, AgentError, AgentInput, AgentIterResult, AgentResult, AgentRunState,
    AgentStreamRecord, AgentStreamResult, OutputPolicy, RunStatus,
};

/// Context event emitted when HITL decisions cannot be applied to a waiting run.
pub const HITL_DECISION_DIAGNOSTIC_EVENT_KIND: &str = "hitl_decision_diagnostic";
const TRACE_METADATA_STATE_KEY: &str = "starweaver.trace_metadata";

/// Context-backed SDK session for repeated runs through one agent.
#[derive(Clone)]
pub struct AgentSession {
    agent: RuntimeAgent,
    context: AgentContext,
    last_run_state: Option<AgentRunState>,
}

/// Per-run SDK overrides composed over a reusable session agent.
#[derive(Clone, Default)]
pub struct AgentRunOptions {
    instructions: Vec<String>,
    model_settings: Option<ModelSettings>,
    request_params: Option<ModelRequestParameters>,
    output_policy: Option<OutputPolicy>,
    tools: ToolRegistry,
    toolsets: Vec<DynToolset>,
    replace_tools: bool,
    trace_metadata: Metadata,
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

    /// Override output behavior for this run.
    #[must_use]
    pub fn output_policy(mut self, policy: OutputPolicy) -> Self {
        self.output_policy = Some(policy);
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
        self.toolsets.push(toolset.clone());
        self
    }

    /// Add many runtime toolsets for this run.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        self.toolsets.extend(toolsets);
        self
    }

    /// Add metadata for this run's model request trace context.
    ///
    /// The metadata is visible to model adapters and copied to the returned run
    /// state, but it is not persisted as session metadata for later runs.
    #[must_use]
    pub fn trace_metadata(mut self, metadata: Metadata) -> Self {
        merge_metadata(&mut self.trace_metadata, metadata);
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
        for toolset in &self.toolsets {
            override_builder = override_builder.toolset(toolset);
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
        if let Some(policy) = self.output_policy {
            override_builder = override_builder.output_policy(policy);
        }
        override_builder.build()
    }
}

/// Host-supplied HITL decisions and deferred results for a waiting run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentHitlResults {
    /// Approval decisions keyed by tool call id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvals: BTreeMap<String, ToolApprovalDecision>,
    /// Results for deferred tool calls.
    #[serde(default, skip_serializing_if = "DeferredToolResults::is_empty")]
    pub deferred_results: DeferredToolResults,
}

impl AgentHitlResults {
    /// Create an empty HITL result set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an approval decision keyed by tool call id.
    #[must_use]
    pub fn approval(
        mut self,
        tool_call_id: impl Into<String>,
        decision: ToolApprovalDecision,
    ) -> Self {
        self.approvals.insert(tool_call_id.into(), decision);
        self
    }

    /// Try to add an approval decision without replacing an existing decision.
    ///
    /// # Errors
    ///
    /// Returns a duplicate-decision error when the tool call id already exists.
    pub fn try_insert_approval(
        &mut self,
        tool_call_id: impl Into<String>,
        decision: ToolApprovalDecision,
    ) -> Result<(), AgentHitlError> {
        let tool_call_id = tool_call_id.into();
        if self.approvals.contains_key(&tool_call_id) {
            return Err(AgentHitlError::DuplicateDecision(tool_call_id));
        }
        self.approvals.insert(tool_call_id, decision);
        Ok(())
    }

    /// Add one deferred result.
    #[must_use]
    pub fn deferred_result(mut self, result: DeferredToolResult) -> Self {
        self.deferred_results.results.push(result);
        self
    }

    /// Replace deferred results.
    #[must_use]
    pub fn deferred_results(mut self, results: DeferredToolResults) -> Self {
        self.deferred_results = results;
        self
    }

    /// Return whether no decisions or deferred results are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.approvals.is_empty() && self.deferred_results.is_empty()
    }
}

/// Host/user interaction for one pending HITL approval.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentHitlUserInteraction {
    /// Pending approval tool call id.
    pub tool_call_id: String,
    /// Whether the user approved execution.
    pub approved: bool,
    /// Actor that supplied the decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// Optional denial or approval reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Optional user input to preprocess into replacement tool arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_input: Option<Value>,
    /// Host metadata attached to the approval decision.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
}

impl AgentHitlUserInteraction {
    /// Build an approved interaction.
    #[must_use]
    pub fn approved(tool_call_id: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            approved: true,
            decided_by: None,
            reason: None,
            user_input: None,
            metadata: Metadata::default(),
        }
    }

    /// Build a denied interaction.
    #[must_use]
    pub fn denied(tool_call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            approved: false,
            decided_by: None,
            reason: Some(reason.into()),
            user_input: None,
            metadata: Metadata::default(),
        }
    }

    /// Attach the deciding actor.
    #[must_use]
    pub fn with_decided_by(mut self, decided_by: impl Into<String>) -> Self {
        self.decided_by = Some(decided_by.into());
        self
    }

    /// Attach a reason.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Attach user input that the approved tool can preprocess.
    #[must_use]
    pub fn with_user_input(mut self, user_input: Value) -> Self {
        self.user_input = Some(user_input);
        self
    }

    /// Attach host metadata.
    #[must_use]
    pub fn with_metadata(mut self, metadata: Metadata) -> Self {
        self.metadata = metadata;
        self
    }
}

/// Tool returns produced after resolving a waiting HITL run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResolvedHitlToolReturns {
    /// Model-visible tool returns ready for the next request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_returns: Vec<ToolReturnPart>,
    /// Approved tool call ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approved: Vec<String>,
    /// Denied tool call ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub denied: Vec<String>,
    /// Completed deferred request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_completed: Vec<String>,
    /// Failed deferred request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_failed: Vec<String>,
    /// Cancelled deferred request ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deferred_cancelled: Vec<String>,
}

impl ResolvedHitlToolReturns {
    /// Return whether the resolution produced no tool returns.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tool_returns.is_empty()
    }
}

/// Errors returned by SDK HITL resolution APIs.
#[derive(Debug, Error)]
pub enum AgentHitlError {
    /// No waiting run state is available in this session.
    #[error("no waiting HITL run state is available")]
    NoWaitingRun,
    /// The supplied run state is not waiting.
    #[error("run {run_id} is not waiting on HITL: {status:?}")]
    NotWaiting {
        /// Run id.
        run_id: String,
        /// Current run status.
        status: RunStatus,
    },
    /// The waiting run has no pending approval or deferred records.
    #[error("waiting run has no pending HITL tool calls")]
    NoPendingHitl,
    /// An approval decision references a tool call that is not pending approval.
    #[error("unknown approval tool call id: {0}")]
    UnknownApproval(String),
    /// A deferred result references a deferred request that is not pending.
    #[error("unknown deferred request id: {0}")]
    UnknownDeferred(String),
    /// A decision or result id appeared more than once.
    #[error("duplicate HITL decision for id: {0}")]
    DuplicateDecision(String),
    /// A pending HITL item was not resolved by the supplied results.
    #[error("missing HITL decisions for pending {kind}: {ids:?}")]
    MissingDecisions {
        /// HITL item kind.
        kind: &'static str,
        /// Missing ids.
        ids: Vec<String>,
    },
    /// The waiting state no longer has the original model tool call.
    #[error("missing original tool call for pending approval: {0}")]
    MissingToolCall(String),
    /// Deferred results must be terminal before they can repair model history.
    #[error("deferred result {id} is not terminal: {status:?}")]
    DeferredResultNotTerminal {
        /// Deferred request id.
        id: String,
        /// Supplied non-terminal status.
        status: SessionExecutionStatus,
    },
    /// Runtime execution failed while executing an approved tool or resumed run.
    #[error(transparent)]
    Agent(#[from] AgentError),
}

impl AgentSession {
    /// Create a session from a runtime agent and a fresh context.
    #[must_use]
    pub fn new(agent: RuntimeAgent) -> Self {
        let mut context = agent.new_context();
        context.set_session_id(SessionId::new());
        Self::with_context(agent, context)
    }

    /// Create a session from a runtime agent and caller-provided context.
    #[must_use]
    pub const fn with_context(agent: RuntimeAgent, context: AgentContext) -> Self {
        Self {
            agent,
            context,
            last_run_state: None,
        }
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
    pub const fn context_mut(&mut self) -> &mut AgentContext {
        &mut self.context
    }

    /// Replace the session context.
    pub fn replace_context(&mut self, context: AgentContext) {
        self.context = context;
        self.last_run_state = None;
    }

    /// Return the most recent run state observed by this session.
    #[must_use]
    pub const fn last_run_state(&self) -> Option<&AgentRunState> {
        self.last_run_state.as_ref()
    }

    /// Convert host/user HITL approval interactions into resumable HITL results.
    ///
    /// Approved interactions with `user_input` call the pending tool's preprocessing hook to
    /// derive replacement arguments and approval metadata before `resume_after_hitl`.
    ///
    /// # Errors
    ///
    /// Returns HITL validation errors for missing waiting state, unknown approvals, or duplicate
    /// decisions.
    pub async fn preprocess_hitl_user_interactions(
        &mut self,
        interactions: impl IntoIterator<Item = AgentHitlUserInteraction>,
    ) -> Result<AgentHitlResults, AgentHitlError> {
        let interactions = interactions.into_iter().collect::<Vec<_>>();
        if interactions.is_empty() {
            return Ok(AgentHitlResults::new());
        }
        let state = self
            .last_run_state
            .clone()
            .ok_or(AgentHitlError::NoWaitingRun)?;
        self.preprocess_hitl_user_interactions_for_state(&state, interactions)
            .await
    }

    /// Convert host/user HITL approval interactions for an explicit waiting state.
    ///
    /// # Errors
    ///
    /// Returns HITL validation errors for non-waiting state, unknown approvals, or duplicate
    /// decisions.
    pub async fn preprocess_hitl_user_interactions_for_state(
        &mut self,
        state: &AgentRunState,
        interactions: impl IntoIterator<Item = AgentHitlUserInteraction>,
    ) -> Result<AgentHitlResults, AgentHitlError> {
        let interactions = interactions.into_iter().collect::<Vec<_>>();
        if interactions.is_empty() {
            return Ok(AgentHitlResults::new());
        }
        if state.status != RunStatus::Waiting {
            return Err(AgentHitlError::NotWaiting {
                run_id: state.run_id.as_str().to_string(),
                status: state.status,
            });
        }
        if state.pending_approval_tool_returns.is_empty() {
            return Err(AgentHitlError::NoPendingHitl);
        }

        let approval_ids = state
            .pending_approval_tool_returns
            .iter()
            .map(|tool_return| tool_return.tool_call_id.clone())
            .collect::<BTreeSet<_>>();
        let tool_calls = pending_tool_calls_by_id(state);
        let tools = self.agent.tools();
        let mut results = AgentHitlResults::new();

        for interaction in interactions {
            let canonical_id = self.canonical_approval_id(state, &interaction.tool_call_id);
            if !approval_ids.contains(&canonical_id) {
                return Err(AgentHitlError::UnknownApproval(canonical_id));
            }
            let decision = if interaction.approved {
                let mut metadata = interaction.metadata;
                if let Some(user_input) = interaction.user_input {
                    let call = tool_calls
                        .get(&canonical_id)
                        .ok_or_else(|| AgentHitlError::MissingToolCall(canonical_id.clone()))?;
                    match self
                        .preprocess_approved_hitl_user_input(state, &tools, call, user_input)
                        .await
                    {
                        Ok(preprocessed) => {
                            let override_arguments = preprocessed.override_arguments;
                            merge_metadata(&mut metadata, preprocessed.metadata);
                            ToolApprovalDecision::Approved {
                                decided_by: interaction.decided_by,
                                reason: interaction.reason,
                                override_arguments,
                                metadata,
                            }
                        }
                        Err(error) => {
                            metadata.insert(
                                "preprocess_error".to_string(),
                                serde_json::json!(error.to_string()),
                            );
                            ToolApprovalDecision::Denied {
                                decided_by: interaction.decided_by,
                                reason: Some("Failed to process user input".to_string()),
                                metadata,
                            }
                        }
                    }
                } else {
                    ToolApprovalDecision::Approved {
                        decided_by: interaction.decided_by,
                        reason: interaction.reason,
                        override_arguments: None,
                        metadata,
                    }
                }
            } else {
                ToolApprovalDecision::Denied {
                    decided_by: interaction.decided_by,
                    reason: interaction.reason,
                    metadata: interaction.metadata,
                }
            };
            results.try_insert_approval(canonical_id, decision)?;
        }

        Ok(results)
    }

    pub(crate) fn record_result(&mut self, result: &AgentResult) {
        self.last_run_state = Some(result.state.clone());
    }

    /// Export curated portable session state for later restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        self.context.export_state()
    }

    /// Export full Starweaver session state for product persistence.
    #[must_use]
    pub fn export_full_state(&self) -> ResumableState {
        self.context.export_full_state()
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

    /// Set the stable logical session affinity identifier.
    pub fn set_session_id(&mut self, session_id: SessionId) {
        self.context.set_session_id(session_id);
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

    /// Restore the active environment provider on the session context.
    pub fn restore_environment(&mut self, provider: DynEnvironmentProvider) {
        self.set_environment(provider);
    }

    /// Return the active environment provider when present.
    #[must_use]
    pub fn environment_provider(&self) -> Option<DynEnvironmentProvider> {
        self.context
            .dependencies
            .get::<EnvironmentHandle>()
            .map(|handle| handle.provider())
    }

    /// Export the active environment provider state.
    ///
    /// # Errors
    ///
    /// Returns provider errors from the active environment. Returns `Ok(None)`
    /// when no environment is attached to the session.
    pub async fn export_environment_state(
        &self,
    ) -> Result<Option<EnvironmentState>, EnvironmentError> {
        let Some(provider) = self.environment_provider() else {
            return Ok(None);
        };
        provider.export_state().await.map(Some)
    }

    /// Inject HITL decisions for the latest waiting run state.
    ///
    /// # Errors
    ///
    /// Returns an error when no waiting run is available, the supplied decisions
    /// do not cover every pending HITL item, or approved tool execution fails at
    /// the runtime boundary.
    pub async fn inject_hitl_results(
        &mut self,
        results: AgentHitlResults,
    ) -> Result<ResolvedHitlToolReturns, AgentHitlError> {
        let state = self
            .last_run_state
            .clone()
            .ok_or(AgentHitlError::NoWaitingRun)?;
        self.inject_hitl_results_for_state(&state, results).await
    }

    /// Inject HITL decisions for an explicitly supplied waiting run state.
    ///
    /// Use this entry point when the process restored context state and a
    /// persisted `AgentRunState` from a durable store.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied state is not waiting, decisions are
    /// incomplete or unknown, or approved tool execution fails at the runtime boundary.
    pub async fn inject_hitl_results_for_state(
        &mut self,
        state: &AgentRunState,
        results: AgentHitlResults,
    ) -> Result<ResolvedHitlToolReturns, AgentHitlError> {
        let resolved = match self.resolve_hitl_results(state, results).await {
            Ok(resolved) => resolved,
            Err(error) => {
                self.publish_hitl_decision_diagnostic(state, &error);
                return Err(error);
            }
        };
        self.context
            .pending_tool_returns
            .extend(resolved.tool_returns.clone());
        self.context
            .publish_event(starweaver_context::AgentEvent::new(
                "hitl_resolved",
                serde_json::json!({
                    "run_id": state.run_id.as_str(),
                    "tool_returns": resolved.tool_returns.len(),
                    "approved": resolved.approved.len(),
                    "denied": resolved.denied.len(),
                    "deferred_completed": resolved.deferred_completed.len(),
                    "deferred_failed": resolved.deferred_failed.len(),
                    "deferred_cancelled": resolved.deferred_cancelled.len(),
                }),
            ));
        self.last_run_state = None;
        Ok(resolved)
    }

    /// Resolve HITL decisions and immediately resume the model loop.
    ///
    /// Passing an empty result set resumes already injected pending tool returns,
    /// which is useful after `inject_hitl_results` followed by full-state restore.
    ///
    /// # Errors
    ///
    /// Returns HITL resolution errors or runtime errors from the resumed run.
    pub async fn resume_after_hitl(
        &mut self,
        results: AgentHitlResults,
    ) -> Result<AgentResult, AgentHitlError> {
        if results.is_empty() {
            if self.context.pending_tool_returns.is_empty() {
                return Err(AgentHitlError::NoWaitingRun);
            }
        } else {
            self.inject_hitl_results(results).await?;
        }
        self.run("").await.map_err(AgentHitlError::from)
    }

    /// Resolve HITL decisions for an explicit waiting state and resume the model loop.
    ///
    /// # Errors
    ///
    /// Returns HITL resolution errors or runtime errors from the resumed run.
    pub async fn resume_after_hitl_for_state(
        &mut self,
        state: &AgentRunState,
        results: AgentHitlResults,
    ) -> Result<AgentResult, AgentHitlError> {
        if results.is_empty() {
            if self.context.pending_tool_returns.is_empty() {
                return Err(AgentHitlError::NoWaitingRun);
            }
        } else {
            self.inject_hitl_results_for_state(state, results).await?;
        }
        self.run("").await.map_err(AgentHitlError::from)
    }

    fn publish_hitl_decision_diagnostic(&mut self, state: &AgentRunState, error: &AgentHitlError) {
        self.context
            .publish_event(starweaver_context::AgentEvent::new(
                HITL_DECISION_DIAGNOSTIC_EVENT_KIND,
                hitl_decision_diagnostic_payload(state, error),
            ));
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

    #[allow(clippy::too_many_lines)]
    async fn resolve_hitl_results(
        &mut self,
        state: &AgentRunState,
        results: AgentHitlResults,
    ) -> Result<ResolvedHitlToolReturns, AgentHitlError> {
        if state.status != RunStatus::Waiting {
            return Err(AgentHitlError::NotWaiting {
                run_id: state.run_id.as_str().to_string(),
                status: state.status,
            });
        }
        if state.pending_approval_tool_returns.is_empty() && state.deferred_tool_returns.is_empty()
        {
            return Err(AgentHitlError::NoPendingHitl);
        }

        let mut approvals = self.canonical_approval_decisions(state, results.approvals)?;
        let mut deferred_results =
            self.collect_deferred_results(state, results.deferred_results)?;
        let approval_ids = state
            .pending_approval_tool_returns
            .iter()
            .map(|tool_return| tool_return.tool_call_id.clone())
            .collect::<BTreeSet<_>>();
        let deferred_ids = self.pending_deferred_ids(state)?;

        for approval_id in approvals.keys() {
            if !approval_ids.contains(approval_id) {
                return Err(AgentHitlError::UnknownApproval(approval_id.clone()));
            }
        }
        for deferred_id in deferred_results.keys() {
            if !deferred_ids.contains_key(deferred_id) {
                return Err(AgentHitlError::UnknownDeferred(deferred_id.clone()));
            }
        }

        let missing_approvals = approval_ids
            .iter()
            .filter(|id| !approvals.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        if !missing_approvals.is_empty() {
            return Err(AgentHitlError::MissingDecisions {
                kind: "approvals",
                ids: missing_approvals,
            });
        }
        let missing_deferred = deferred_ids
            .keys()
            .filter(|id| !deferred_results.contains_key(*id))
            .cloned()
            .collect::<Vec<_>>();
        if !missing_deferred.is_empty() {
            return Err(AgentHitlError::MissingDecisions {
                kind: "deferred_tools",
                ids: missing_deferred,
            });
        }

        let tool_calls = pending_tool_calls_by_id(state);
        let mut resolved = ResolvedHitlToolReturns::default();
        for pending_return in &state.pending_approval_tool_returns {
            let Some(decision) = approvals.remove(&pending_return.tool_call_id) else {
                continue;
            };
            match decision {
                ToolApprovalDecision::Approved {
                    decided_by,
                    reason,
                    override_arguments,
                    mut metadata,
                } => {
                    if let Some(decided_by) = decided_by {
                        metadata.insert("decided_by".to_string(), serde_json::json!(decided_by));
                    }
                    if let Some(reason) = reason {
                        metadata.insert("reason".to_string(), serde_json::json!(reason));
                    }
                    let call = tool_calls
                        .get(&pending_return.tool_call_id)
                        .ok_or_else(|| {
                            AgentHitlError::MissingToolCall(pending_return.tool_call_id.clone())
                        })?;
                    let mut tool_return = self
                        .execute_approved_tool_call(
                            state,
                            call,
                            ToolApprovalState::Approved {
                                override_arguments,
                                metadata,
                            },
                        )
                        .await;
                    tool_return
                        .metadata
                        .entry("hitl_state".to_string())
                        .or_insert_with(|| serde_json::json!("resolved"));
                    tool_return
                        .metadata
                        .entry("approval_state".to_string())
                        .or_insert_with(|| serde_json::json!("approved"));
                    resolved.approved.push(pending_return.tool_call_id.clone());
                    resolved.tool_returns.push(tool_return);
                }
                ToolApprovalDecision::Denied {
                    decided_by,
                    reason,
                    metadata,
                } => {
                    let tool_return =
                        denied_tool_return(pending_return, decided_by, reason, metadata);
                    resolved.denied.push(pending_return.tool_call_id.clone());
                    resolved.tool_returns.push(tool_return);
                }
            }
        }

        for (deferred_id, pending_return) in deferred_ids {
            let Some(result) = deferred_results.remove(&deferred_id) else {
                continue;
            };
            let tool_return = deferred_result_tool_return(&pending_return, result)?;
            match tool_return
                .metadata
                .get("deferred_status")
                .and_then(Value::as_str)
            {
                Some("completed") => resolved.deferred_completed.push(deferred_id),
                Some("failed") => resolved.deferred_failed.push(deferred_id),
                Some("cancelled") => resolved.deferred_cancelled.push(deferred_id),
                _ => {}
            }
            resolved.tool_returns.push(tool_return);
        }

        Ok(resolved)
    }

    fn canonical_approval_decisions(
        &self,
        state: &AgentRunState,
        approvals: BTreeMap<String, ToolApprovalDecision>,
    ) -> Result<BTreeMap<String, ToolApprovalDecision>, AgentHitlError> {
        let mut canonical = BTreeMap::new();
        for (approval_id, decision) in approvals {
            let canonical_id = self.canonical_approval_id(state, &approval_id);
            if canonical.insert(canonical_id.clone(), decision).is_some() {
                return Err(AgentHitlError::DuplicateDecision(canonical_id));
            }
        }
        Ok(canonical)
    }

    fn collect_deferred_results(
        &self,
        state: &AgentRunState,
        results: DeferredToolResults,
    ) -> Result<BTreeMap<String, DeferredToolResult>, AgentHitlError> {
        let mut by_id = BTreeMap::new();
        for mut result in results.results {
            result.deferred_id = self.canonical_deferred_id(state, &result.deferred_id);
            let id = result.deferred_id.clone();
            if by_id.insert(id.clone(), result).is_some() {
                return Err(AgentHitlError::DuplicateDecision(id));
            }
        }
        Ok(by_id)
    }

    fn canonical_deferred_id(&self, state: &AgentRunState, deferred_id: &str) -> String {
        let prefix = format!("deferred_{}_", state.run_id.as_str());
        let Some(tool_call_id) = deferred_id.strip_prefix(&prefix) else {
            return deferred_id.to_string();
        };
        format!("{prefix}{}", self.canonical_tool_call_id(tool_call_id))
    }

    fn canonical_approval_id(&self, state: &AgentRunState, approval_id: &str) -> String {
        let prefix = format!("approval_{}_", state.run_id.as_str());
        let tool_call_id = approval_id.strip_prefix(&prefix).unwrap_or(approval_id);
        self.canonical_tool_call_id(tool_call_id)
    }

    fn canonical_tool_call_id(&self, tool_call_id: &str) -> String {
        self.context
            .tool_id_wrapper
            .tool_call_maps
            .get(tool_call_id)
            .cloned()
            .unwrap_or_else(|| tool_call_id.to_string())
    }

    async fn preprocess_approved_hitl_user_input(
        &mut self,
        state: &AgentRunState,
        tools: &ToolRegistry,
        call: &ToolCallPart,
        user_input: Value,
    ) -> Result<ToolUserInputPreprocessResult, ToolError> {
        let Some(tool) = tools.get(&call.name) else {
            return Err(ToolError::NotFound(call.name.clone()));
        };
        let context_handle = AgentContextHandle::new(self.context.clone());
        let mut tool_dependencies = self.context.dependencies.clone();
        tool_dependencies.insert(self.context.clone());
        tool_dependencies.insert(context_handle.clone());
        let mut metadata = Metadata::default();
        metadata.insert(
            "tool_call_id".to_string(),
            serde_json::json!(call.id.clone()),
        );
        metadata.insert(
            "tool_name".to_string(),
            serde_json::json!(call.name.clone()),
        );
        let mut tool_context = ToolContext::new(
            state.run_id.clone(),
            state.conversation_id.clone(),
            state.run_step,
        )
        .with_dependencies(tool_dependencies)
        .with_trace_context(self.context.trace_context.clone())
        .with_retry_budget(0, tools.max_retries_for(&call.name));
        tool_context.metadata = metadata;
        let result = tool.preprocess_user_input(tool_context, user_input).await;
        self.absorb_tool_context_handle(&context_handle);
        result
    }

    async fn execute_approved_tool_call(
        &mut self,
        state: &AgentRunState,
        call: &ToolCallPart,
        approval: ToolApprovalState,
    ) -> ToolReturnPart {
        self.context.current_run_step = state.run_step;
        let tools = match self
            .agent
            .prepare_tools_for_context(&mut self.context)
            .await
        {
            Ok(tools) => tools,
            Err(error) => {
                self.agent
                    .close_toolsets_for_context(&mut self.context)
                    .await;
                return error_return(
                    call,
                    &ToolError::Execution {
                        tool: call.name.clone(),
                        message: error.to_string(),
                    },
                );
            }
        };
        let context_handle = AgentContextHandle::new(self.context.clone());
        let mut tool_dependencies = self.context.dependencies.clone();
        tool_dependencies.insert(self.context.clone());
        tool_dependencies.insert(context_handle.clone());
        let tool_context = ToolContext::new(
            state.run_id.clone(),
            state.conversation_id.clone(),
            state.run_step,
        )
        .with_dependencies(tool_dependencies)
        .with_trace_context(self.context.trace_context.clone())
        .with_retry_budget(0, tools.max_retries_for(&call.name))
        .with_approval(approval);
        let started_at = std::time::Instant::now();
        let mut tool_return = tools.execute_call(tool_context, call).await;
        self.absorb_tool_context_handle(&context_handle);
        self.agent
            .close_toolsets_for_context(&mut self.context)
            .await;
        if !tool_return.is_error {
            self.context.usage.tool_calls = self.context.usage.tool_calls.saturating_add(1);
        }
        let duration = started_at.elapsed();
        let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        tool_return
            .metadata
            .insert("duration_ms".to_string(), serde_json::json!(duration_ms));
        tool_return.metadata.insert(
            "duration_seconds".to_string(),
            serde_json::json!(duration.as_secs_f64()),
        );
        tool_return
    }

    fn absorb_tool_context_handle(&mut self, handle: &AgentContextHandle) {
        let snapshot = handle.snapshot();
        self.context.usage = snapshot.usage;
        self.context.notes = snapshot.notes;
        self.context.state = snapshot.state;
        self.context.task_manager = snapshot.task_manager;
        self.context.events = snapshot.events;
        self.context.messages = snapshot.messages;
        self.context.metadata = snapshot.metadata;
        self.context.deferred_tool_metadata = snapshot.deferred_tool_metadata;
        self.context.agent_registry = snapshot.agent_registry;
        self.context.subagent_history = snapshot.subagent_history;
        self.context.auto_load_files = snapshot.auto_load_files;
        self.context.approval_required_tools = snapshot.approval_required_tools;
        self.context.approval_required_mcp_servers = snapshot.approval_required_mcp_servers;
        self.context.tool_search_loaded_tools = snapshot.tool_search_loaded_tools;
        self.context.tool_search_loaded_namespaces = snapshot.tool_search_loaded_namespaces;
        self.context.context_manage_tool_names = snapshot.context_manage_tool_names;
        self.context.tool_tags = snapshot.tool_tags;
        self.context.wrapper_metadata = snapshot.wrapper_metadata;
    }

    fn pending_deferred_ids(
        &self,
        state: &AgentRunState,
    ) -> Result<BTreeMap<String, ToolReturnPart>, AgentHitlError> {
        let session_id = self.context.session_id.clone().unwrap_or_default();
        let mut ids = BTreeMap::new();
        for tool_return in &state.deferred_tool_returns {
            let input = ToolReturnRecordInput::new(
                &session_id,
                &state.run_id,
                &tool_return.tool_call_id,
                &tool_return.name,
                &tool_return.metadata,
            )
            .with_trace_context(&self.context.trace_context);
            let Some(record) = DeferredToolRecord::from_tool_return(&input) else {
                continue;
            };
            if ids
                .insert(record.deferred_id.clone(), tool_return.clone())
                .is_some()
            {
                return Err(AgentHitlError::DuplicateDecision(record.deferred_id));
            }
        }
        Ok(ids)
    }

    /// Run the session agent with the session context.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run(&mut self, prompt: impl Into<AgentInput>) -> Result<AgentResult, AgentError> {
        let result = self
            .agent
            .run_with_context(prompt, &mut self.context)
            .await?;
        self.record_result(&result);
        Ok(result)
    }

    /// Run with per-run SDK overrides composed over the reusable session agent.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_with_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentResult, AgentError> {
        let trace_metadata = options.trace_metadata.clone();
        let original_metadata = apply_run_trace_metadata(&mut self.context, &trace_metadata);
        let result = options
            .apply(&self.agent)
            .run_with_context(prompt, &mut self.context)
            .await;
        restore_context_metadata(&mut self.context, original_metadata);
        let mut result = result?;
        attach_result_trace_metadata(&mut result, &trace_metadata);
        self.record_result(&result);
        Ok(result)
    }

    /// Run the session agent and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_stream(
        &mut self,
        prompt: impl Into<AgentInput>,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::<AgentStreamRecord>::new();
        let result = self
            .agent
            .run_with_context_and_stream_events(prompt, &mut self.context, &mut events)
            .await?;
        self.record_result(&result);
        Ok(AgentStreamResult { result, events })
    }

    /// Run with per-run SDK overrides and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_stream_with_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::<AgentStreamRecord>::new();
        let trace_metadata = options.trace_metadata.clone();
        let original_metadata = apply_run_trace_metadata(&mut self.context, &trace_metadata);
        let result = options
            .apply(&self.agent)
            .run_with_context_and_stream_events(prompt, &mut self.context, &mut events)
            .await;
        restore_context_metadata(&mut self.context, original_metadata);
        let mut result = result?;
        attach_result_trace_metadata(&mut result, &trace_metadata);
        self.record_result(&result);
        Ok(AgentStreamResult { result, events })
    }

    /// Start a live stream run from the current session context.
    ///
    /// The returned handle yields stream records while the run is still active.
    /// Use `AgentStreamHandle::finish_into_session` to write the completed
    /// context back into this session.
    #[must_use]
    pub fn stream(&mut self, prompt: impl Into<AgentInput>) -> AgentStreamHandle {
        start_session_stream(self.agent.clone(), self.context.clone(), prompt.into())
    }

    /// Try to start a live stream run from the current session context.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream(
        &mut self,
        prompt: impl Into<AgentInput>,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        try_start_session_stream(self.agent.clone(), self.context.clone(), prompt.into())
    }

    /// Start a live stream run with explicit stream delivery options.
    ///
    /// The returned handle yields stream records while the run is still active.
    /// Use `AgentStreamHandle::finish_into_session` to write the completed
    /// context back into this session.
    #[must_use]
    pub fn stream_with_stream_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        stream_options: AgentStreamOptions,
    ) -> AgentStreamHandle {
        start_session_stream_with_options(
            self.agent.clone(),
            self.context.clone(),
            prompt.into(),
            stream_options,
        )
    }

    /// Try to start a live stream run with explicit stream delivery options.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_stream_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        stream_options: AgentStreamOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        try_start_session_stream_with_options(
            self.agent.clone(),
            self.context.clone(),
            prompt.into(),
            stream_options,
        )
    }

    /// Start a live stream run with per-run SDK overrides.
    ///
    /// The returned handle yields stream records while the run is still active.
    /// Use `AgentStreamHandle::finish_into_session` to write the completed
    /// context back into this session.
    #[must_use]
    pub fn stream_with_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
    ) -> AgentStreamHandle {
        let trace_metadata = options.trace_metadata.clone();
        let mut context = self.context.clone();
        let original_metadata = apply_run_trace_metadata(&mut context, &trace_metadata);
        start_session_stream(options.apply(&self.agent), context, prompt.into())
            .with_optional_temporary_trace_metadata(original_metadata, trace_metadata)
    }

    /// Try to start a live stream run with per-run SDK overrides.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        let trace_metadata = options.trace_metadata.clone();
        let mut context = self.context.clone();
        let original_metadata = apply_run_trace_metadata(&mut context, &trace_metadata);
        Ok(
            try_start_session_stream(options.apply(&self.agent), context, prompt.into())?
                .with_optional_temporary_trace_metadata(original_metadata, trace_metadata),
        )
    }

    /// Start a live stream run with both run overrides and stream delivery options.
    ///
    /// The returned handle yields stream records while the run is still active.
    /// Use `AgentStreamHandle::finish_into_session` to write the completed
    /// context back into this session.
    #[must_use]
    pub fn stream_with_run_and_stream_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
        stream_options: AgentStreamOptions,
    ) -> AgentStreamHandle {
        let trace_metadata = options.trace_metadata.clone();
        let mut context = self.context.clone();
        let original_metadata = apply_run_trace_metadata(&mut context, &trace_metadata);
        start_session_stream_with_options(
            options.apply(&self.agent),
            context,
            prompt.into(),
            stream_options,
        )
        .with_optional_temporary_trace_metadata(original_metadata, trace_metadata)
    }

    /// Try to start a live stream run with both run overrides and stream delivery options.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_run_and_stream_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
        stream_options: AgentStreamOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        let trace_metadata = options.trace_metadata.clone();
        let mut context = self.context.clone();
        let original_metadata = apply_run_trace_metadata(&mut context, &trace_metadata);
        Ok(try_start_session_stream_with_options(
            options.apply(&self.agent),
            context,
            prompt.into(),
            stream_options,
        )?
        .with_optional_temporary_trace_metadata(original_metadata, trace_metadata))
    }
}

fn hitl_decision_diagnostic_payload(state: &AgentRunState, error: &AgentHitlError) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "run_id".to_string(),
        serde_json::json!(state.run_id.as_str()),
    );
    payload.insert("message".to_string(), serde_json::json!(error.to_string()));
    match error {
        AgentHitlError::NoWaitingRun => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("no_waiting_run"),
            );
        }
        AgentHitlError::NotWaiting { run_id, status } => {
            payload.insert("error_kind".to_string(), serde_json::json!("not_waiting"));
            payload.insert("referenced_run_id".to_string(), serde_json::json!(run_id));
            payload.insert(
                "status".to_string(),
                serde_json::json!(format!("{status:?}")),
            );
        }
        AgentHitlError::NoPendingHitl => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("no_pending_hitl"),
            );
        }
        AgentHitlError::UnknownApproval(id) => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("unknown_approval"),
            );
            payload.insert("approval_id".to_string(), serde_json::json!(id));
        }
        AgentHitlError::UnknownDeferred(id) => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("unknown_deferred"),
            );
            payload.insert("deferred_id".to_string(), serde_json::json!(id));
        }
        AgentHitlError::DuplicateDecision(id) => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("duplicate_decision"),
            );
            payload.insert("decision_id".to_string(), serde_json::json!(id));
        }
        AgentHitlError::MissingDecisions { kind, ids } => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("missing_decisions"),
            );
            payload.insert("decision_kind".to_string(), serde_json::json!(kind));
            payload.insert("ids".to_string(), serde_json::json!(ids));
        }
        AgentHitlError::MissingToolCall(id) => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("missing_tool_call"),
            );
            payload.insert("tool_call_id".to_string(), serde_json::json!(id));
        }
        AgentHitlError::DeferredResultNotTerminal { id, status } => {
            payload.insert(
                "error_kind".to_string(),
                serde_json::json!("deferred_result_not_terminal"),
            );
            payload.insert("deferred_id".to_string(), serde_json::json!(id));
            payload.insert(
                "status".to_string(),
                serde_json::json!(format!("{status:?}")),
            );
        }
        AgentHitlError::Agent(error) => {
            payload.insert("error_kind".to_string(), serde_json::json!("agent_error"));
            payload.insert(
                "agent_error".to_string(),
                serde_json::json!(error.to_string()),
            );
        }
    }
    Value::Object(payload)
}

fn pending_tool_calls_by_id(state: &AgentRunState) -> BTreeMap<String, ToolCallPart> {
    let mut calls = BTreeMap::new();
    if let Some(response) = &state.latest_response {
        for call in response.tool_calls() {
            calls.insert(call.id.clone(), call);
        }
    }
    for call in &state.pending_tool_calls {
        calls.insert(call.id.clone(), call.clone());
    }
    calls
}

fn merge_metadata(target: &mut Metadata, incoming: Metadata) {
    for (key, value) in incoming {
        target.insert(key, value);
    }
}

fn apply_run_trace_metadata(
    context: &mut AgentContext,
    trace_metadata: &Metadata,
) -> Option<Metadata> {
    if trace_metadata.is_empty() {
        return None;
    }
    let original = context.metadata.clone();
    merge_metadata(&mut context.metadata, trace_metadata.clone());
    Some(original)
}

fn restore_context_metadata(context: &mut AgentContext, original_metadata: Option<Metadata>) {
    if let Some(original_metadata) = original_metadata {
        context.metadata = original_metadata;
    }
}

fn attach_result_trace_metadata(result: &mut AgentResult, trace_metadata: &Metadata) {
    if !trace_metadata.is_empty() {
        result.state.metadata.insert(
            TRACE_METADATA_STATE_KEY.to_string(),
            Value::Object(trace_metadata.clone()),
        );
    }
}

fn denied_tool_return(
    pending_return: &ToolReturnPart,
    decided_by: Option<String>,
    reason: Option<String>,
    decision_metadata: Metadata,
) -> ToolReturnPart {
    let mut metadata = Metadata::default();
    metadata.insert("hitl_state".to_string(), serde_json::json!("resolved"));
    metadata.insert("approval_state".to_string(), serde_json::json!("denied"));
    if let Some(decided_by) = decided_by {
        metadata.insert("decided_by".to_string(), serde_json::json!(decided_by));
    }
    if !decision_metadata.is_empty() {
        metadata.insert(
            "approval_metadata".to_string(),
            Value::Object(decision_metadata),
        );
    }
    let message = reason.unwrap_or_else(|| "User denied the tool call.".to_string());
    ToolReturnPart::new(
        pending_return.tool_call_id.clone(),
        pending_return.name.clone(),
        serde_json::json!({
            "kind": "approval_denied",
            "message": message,
            "tool_call_id": pending_return.tool_call_id.clone(),
            "tool_name": pending_return.name.clone(),
        }),
    )
    .with_error(true)
    .with_metadata(metadata)
}

fn deferred_result_tool_return(
    pending_return: &ToolReturnPart,
    result: DeferredToolResult,
) -> Result<ToolReturnPart, AgentHitlError> {
    let status = result.status;
    let status_name = execution_status_name(status);
    let mut metadata = result.metadata;
    metadata.insert("hitl_state".to_string(), serde_json::json!("resolved"));
    metadata.insert(
        "deferred_id".to_string(),
        serde_json::json!(result.deferred_id),
    );
    metadata.insert(
        "deferred_status".to_string(),
        serde_json::json!(status_name),
    );
    match status {
        SessionExecutionStatus::Completed => Ok(ToolReturnPart::new(
            pending_return.tool_call_id.clone(),
            pending_return.name.clone(),
            result.response,
        )
        .with_metadata(metadata)),
        SessionExecutionStatus::Failed | SessionExecutionStatus::Cancelled => {
            let kind = if status == SessionExecutionStatus::Failed {
                "deferred_failed"
            } else {
                "deferred_cancelled"
            };
            Ok(ToolReturnPart::new(
                pending_return.tool_call_id.clone(),
                pending_return.name.clone(),
                serde_json::json!({
                    "kind": kind,
                    "status": status_name,
                    "response": result.response,
                }),
            )
            .with_error(true)
            .with_metadata(metadata))
        }
        SessionExecutionStatus::Pending
        | SessionExecutionStatus::Running
        | SessionExecutionStatus::Waiting => Err(AgentHitlError::DeferredResultNotTerminal {
            id: metadata
                .get("deferred_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            status,
        }),
    }
}

const fn execution_status_name(status: SessionExecutionStatus) -> &'static str {
    match status {
        SessionExecutionStatus::Pending => "pending",
        SessionExecutionStatus::Running => "running",
        SessionExecutionStatus::Waiting => "waiting",
        SessionExecutionStatus::Completed => "completed",
        SessionExecutionStatus::Failed => "failed",
        SessionExecutionStatus::Cancelled => "cancelled",
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
        prompt: impl Into<AgentInput>,
    ) -> Result<AgentIterResult, AgentError> {
        let result = self
            .agent
            .run_with_context_iter(prompt, &mut self.context)
            .await?;
        self.record_result(&result.result);
        Ok(result)
    }

    /// Run with per-run SDK overrides and collect compact iteration inspection records.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_iter_with_options(
        &mut self,
        prompt: impl Into<AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentIterResult, AgentError> {
        let trace_metadata = options.trace_metadata.clone();
        let original_metadata = apply_run_trace_metadata(&mut self.context, &trace_metadata);
        let result = options
            .apply(&self.agent)
            .run_with_context_iter(prompt, &mut self.context)
            .await;
        restore_context_metadata(&mut self.context, original_metadata);
        let mut result = result?;
        attach_result_trace_metadata(&mut result.result, &trace_metadata);
        self.record_result(&result.result);
        Ok(result)
    }
}
