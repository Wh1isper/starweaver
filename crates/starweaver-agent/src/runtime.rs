//! SDK runtime builder and owned runtime facade.

use std::sync::Arc;

use chrono::Utc;
use starweaver_context::{AgentContext, ModelConfig, ResumableState, SecurityConfig, ToolConfig};
use starweaver_core::{RunId, SessionId};
use starweaver_environment::{
    DynEnvironmentProvider, EnvironmentError, EnvironmentProviderFactoryRegistry, EnvironmentState,
    ResourceRestoreFactoryRegistry,
};
use starweaver_model::{ContentPart, ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_runtime::{
    AgentCapability, AgentError, AgentExecutorError, AgentResult, AgentRuntimePolicy,
    AgentStreamRecord, AgentStreamResult, OutputFunction, OutputPolicy, OutputSchema,
    OutputValidator, RunStatus,
};
use starweaver_session::{
    InputPart, RunRecord, RunStatus as SessionRunStatus, SessionRecord, SessionResumeSnapshot,
    SessionStore, SessionStoreError, SessionStoreExecutor, ToolReturnRecordInput,
};
use starweaver_stream::{
    DefaultDisplayMessageProjector, DisplayMessageProjector, DisplayProjectionContext, ReplayEvent,
    ReplayEventKind, ReplayEventLog, ReplayScope, StreamArchive, StreamTerminalMarker,
};
use starweaver_tools::{DynTool, DynToolset, ToolRegistry};
use starweaver_usage::UsageLimits;
use thiserror::Error;

use crate::{
    AgentApp, AgentBuilder, AgentHitlError, AgentHitlResults, AgentRunOptions, AgentSession,
    AgentStreamCompletion, AgentStreamError, AgentStreamHandle, AgentStreamOptions, MediaUploader,
    SkillRegistry, SkillScanReport, SubagentConfig, SubagentDelegationMode, SubagentRegistry,
};

/// Errors returned by durable SDK runtime orchestration.
#[derive(Debug, Error)]
pub enum AgentDurabilityError {
    /// Runtime was not built with a session store.
    #[error("agent runtime is not bound to a durable session store")]
    MissingSessionStore,
    /// Resume snapshot did not contain a checkpointable waiting run state.
    #[error("resume snapshot for {session_id}:{run_id} has no checkpoint state")]
    MissingCheckpointState {
        /// Session id.
        session_id: String,
        /// Run id.
        run_id: String,
    },
    /// Session store failed.
    #[error(transparent)]
    SessionStore(#[from] SessionStoreError),
    /// Stream archive or replay log failed.
    #[error(transparent)]
    Replay(#[from] starweaver_stream::ReplayError),
    /// HITL resolution failed.
    #[error(transparent)]
    Hitl(#[from] AgentHitlError),
    /// Agent execution failed.
    #[error(transparent)]
    Agent(#[from] AgentError),
    /// Live stream failed before it could be persisted.
    #[error(transparent)]
    Stream(#[from] AgentStreamError),
}

#[derive(Clone)]
struct AgentDurability {
    session_id: SessionId,
    session_store: Arc<dyn SessionStore>,
    stream_archive: Option<Arc<dyn StreamArchive>>,
    replay_event_log: Option<Arc<dyn ReplayEventLog>>,
    projector: Arc<dyn DisplayMessageProjector>,
}

impl AgentDurability {
    fn new(session_id: SessionId, session_store: Arc<dyn SessionStore>) -> Self {
        Self {
            session_id,
            session_store,
            stream_archive: None,
            replay_event_log: None,
            projector: Arc::new(DefaultDisplayMessageProjector),
        }
    }

    fn stream_archive(mut self, stream_archive: Arc<dyn StreamArchive>) -> Self {
        self.stream_archive = Some(stream_archive);
        self
    }

    fn replay_event_log(mut self, replay_event_log: Arc<dyn ReplayEventLog>) -> Self {
        self.replay_event_log = Some(replay_event_log);
        self
    }
}

/// Builder for an owned SDK runtime.
pub struct AgentRuntimeBuilder {
    builder: AgentBuilder,
    context: Option<AgentContext>,
    state: Option<ResumableState>,
    environment: Option<DynEnvironmentProvider>,
    security: Option<SecurityConfig>,
    durable_session_id: Option<SessionId>,
    session_store: Option<Arc<dyn SessionStore>>,
    stream_archive: Option<Arc<dyn StreamArchive>>,
    replay_event_log: Option<Arc<dyn ReplayEventLog>>,
}

impl AgentRuntimeBuilder {
    /// Create a runtime builder from a model adapter.
    #[must_use]
    pub fn new(model: Arc<dyn ModelAdapter>) -> Self {
        Self {
            builder: AgentBuilder::new(model),
            context: None,
            state: None,
            environment: None,
            security: None,
            durable_session_id: None,
            session_store: None,
            stream_archive: None,
            replay_event_log: None,
        }
    }

    /// Start from an existing agent builder.
    #[must_use]
    pub fn from_builder(builder: AgentBuilder) -> Self {
        Self {
            builder,
            context: None,
            state: None,
            environment: None,
            security: None,
            durable_session_id: None,
            session_store: None,
            stream_archive: None,
            replay_event_log: None,
        }
    }

    /// Set the stable agent id used by the owned runtime's default session.
    #[must_use]
    pub fn agent_id(mut self, agent_id: impl Into<String>) -> Self {
        self.builder = self.builder.agent_id(agent_id);
        self
    }

    /// Set the human-readable agent name used by the owned runtime's default session.
    #[must_use]
    pub fn agent_name(mut self, agent_name: impl Into<String>) -> Self {
        self.builder = self.builder.agent_name(agent_name);
        self
    }

    /// Set both the stable agent id and human-readable name.
    #[must_use]
    pub fn agent_identity(
        mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
    ) -> Self {
        self.builder = self.builder.agent_identity(agent_id, agent_name);
        self
    }

    /// Set the model used by the default compacting filter.
    #[must_use]
    pub fn compact_model(mut self, model: Arc<dyn ModelAdapter>) -> Self {
        self.builder = self.builder.compact_model(model);
        self
    }

    /// Set model settings used by the default compacting filter.
    #[must_use]
    pub fn compact_model_settings(mut self, settings: ModelSettings) -> Self {
        self.builder = self.builder.compact_model_settings(settings);
        self
    }

    /// Set request parameters used by the default compacting filter.
    #[must_use]
    pub fn compact_request_params(mut self, params: ModelRequestParameters) -> Self {
        self.builder = self.builder.compact_request_params(params);
        self
    }

    /// Add a static instruction.
    #[must_use]
    pub fn instruction(mut self, instruction: impl Into<String>) -> Self {
        self.builder = self.builder.instruction(instruction);
        self
    }

    /// Set model settings.
    #[must_use]
    pub fn model_settings(mut self, settings: ModelSettings) -> Self {
        self.builder = self.builder.model_settings(settings);
        self
    }

    /// Set provider-neutral request parameters.
    #[must_use]
    pub fn request_params(mut self, params: ModelRequestParameters) -> Self {
        self.builder = self.builder.request_params(params);
        self
    }

    /// Set structured output schema.
    #[must_use]
    pub fn output_schema(mut self, schema: OutputSchema) -> Self {
        self.builder = self.builder.output_schema(schema);
        self
    }

    /// Apply a complete output policy.
    #[must_use]
    pub fn output_policy(mut self, policy: OutputPolicy) -> Self {
        self.builder = self.builder.output_policy(policy);
        self
    }

    /// Add a structured output validator.
    #[must_use]
    pub fn output_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.builder = self.builder.output_validator(validator);
        self
    }

    /// Add an output function.
    #[must_use]
    pub fn output_function(mut self, function: Arc<dyn OutputFunction>) -> Self {
        self.builder = self.builder.output_function(function);
        self
    }

    /// Set usage limits.
    #[must_use]
    pub fn usage_limits(mut self, limits: UsageLimits) -> Self {
        self.builder = self.builder.usage_limits(limits);
        self
    }

    /// Set the full model config stored on `AgentContext` at run time.
    #[must_use]
    pub fn model_config(mut self, model_config: ModelConfig) -> Self {
        self.builder = self.builder.model_config(model_config);
        self
    }

    /// Set the tool config stored on `AgentContext` at run time.
    #[must_use]
    pub fn tool_config(mut self, tool_config: ToolConfig) -> Self {
        self.builder = self.builder.tool_config(tool_config);
        self
    }

    /// Attach SDK security config to the initial context.
    #[must_use]
    pub fn security(mut self, security: SecurityConfig) -> Self {
        self.security = Some(security);
        self
    }

    /// Set the model context window exposed to runtime context instructions.
    #[must_use]
    pub fn context_window(mut self, context_window: u64) -> Self {
        self.builder = self.builder.context_window(context_window);
        self
    }

    /// Add one runtime tool.
    #[must_use]
    pub fn tool(mut self, tool: DynTool) -> Self {
        self.builder = self.builder.tool(tool);
        self
    }

    /// Add one runtime toolset.
    #[must_use]
    pub fn toolset(mut self, toolset: &DynToolset) -> Self {
        self.builder = self.builder.toolset(toolset);
        self
    }

    /// Add many runtime toolsets in registration order.
    #[must_use]
    pub fn toolsets(mut self, toolsets: impl IntoIterator<Item = DynToolset>) -> Self {
        self.builder = self.builder.toolsets(toolsets);
        self
    }

    /// Require HITL approval for matching tools in registered toolsets.
    #[must_use]
    pub fn approval_required_tools(
        mut self,
        tools: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.builder = self.builder.approval_required_tools(tools);
        self
    }

    /// Set the complete runtime tool registry.
    #[must_use]
    pub fn tool_registry(mut self, tools: ToolRegistry) -> Self {
        self.builder = self.builder.tool_registry(tools);
        self
    }

    /// Set runtime tool retry default.
    #[must_use]
    pub fn tool_retries(mut self, max_retries: usize) -> Self {
        self.builder = self.builder.tool_retries(max_retries);
        self
    }

    /// Install scanned skill summaries and relaxed skill markdown discovery.
    #[must_use]
    pub fn skills(mut self, registry: SkillRegistry) -> Self {
        self.builder = self.builder.skills(registry);
        self
    }

    /// Install scanned skills from a lenient report and publish scan diagnostics at run start.
    #[must_use]
    pub fn skills_report(mut self, report: SkillScanReport) -> Self {
        self.builder = self.builder.skills_report(report);
        self
    }

    /// Add a capability hook.
    #[must_use]
    pub fn capability(mut self, capability: Arc<dyn AgentCapability>) -> Self {
        self.builder = self.builder.capability(capability);
        self
    }

    /// Add a subagent configuration.
    #[must_use]
    pub fn subagent(mut self, subagent: SubagentConfig) -> Self {
        self.builder = self.builder.subagent(subagent);
        self
    }

    /// Set the SDK-level subagent registry.
    #[must_use]
    pub fn subagent_registry(mut self, registry: SubagentRegistry) -> Self {
        self.builder = self.builder.subagent_registry(registry);
        self
    }

    /// Set how registered subagents are exposed as model-callable tools.
    #[must_use]
    pub fn subagent_delegation_mode(mut self, mode: SubagentDelegationMode) -> Self {
        self.builder = self.builder.subagent_delegation_mode(mode);
        self
    }

    /// Set runtime trace recorder.
    #[must_use]
    pub fn trace_recorder(mut self, recorder: starweaver_runtime::DynTraceRecorder) -> Self {
        self.builder = self.builder.trace_recorder(recorder);
        self
    }

    /// Set media uploader used by default filters.
    #[must_use]
    pub fn media_uploader(mut self, uploader: Arc<dyn MediaUploader>) -> Self {
        self.builder = self.builder.media_uploader(uploader);
        self
    }

    /// Set runtime policy.
    #[must_use]
    pub fn policy(mut self, policy: AgentRuntimePolicy) -> Self {
        self.builder = self.builder.policy(policy);
        self
    }

    /// Set the durable session id used by session-store backed runs.
    #[must_use]
    pub fn durable_session_id(mut self, session_id: SessionId) -> Self {
        self.durable_session_id = Some(session_id);
        self
    }

    /// Bind a durable session store to the owned runtime.
    #[must_use]
    pub fn session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    /// Bind a stream archive for raw runtime records and projected display messages.
    #[must_use]
    pub fn stream_archive(mut self, archive: Arc<dyn StreamArchive>) -> Self {
        self.stream_archive = Some(archive);
        self
    }

    /// Bind a replay event log for projected display and terminal replay events.
    #[must_use]
    pub fn replay_event_log(mut self, log: Arc<dyn ReplayEventLog>) -> Self {
        self.replay_event_log = Some(log);
        self
    }

    /// Use a caller-provided context.
    #[must_use]
    pub fn context(mut self, context: AgentContext) -> Self {
        self.context = Some(context);
        self.state = None;
        self
    }

    /// Restore the runtime from exported state.
    #[must_use]
    pub fn state(mut self, state: ResumableState) -> Self {
        self.state = Some(state);
        self.context = None;
        self
    }

    /// Attach an environment provider to the runtime session.
    #[must_use]
    pub fn environment(mut self, environment: DynEnvironmentProvider) -> Self {
        self.environment = Some(environment);
        self
    }

    /// Return the inner reusable builder for advanced customization.
    #[must_use]
    pub fn into_builder(self) -> AgentBuilder {
        self.builder
    }

    /// Build an owned runtime.
    #[must_use]
    pub fn build(mut self) -> AgentRuntime {
        let durability = if let Some(store) = self.session_store.clone() {
            let session_id = self
                .durable_session_id
                .clone()
                .or_else(|| {
                    self.context
                        .as_ref()
                        .and_then(|context| context.session_id().cloned())
                })
                .or_else(|| {
                    self.state
                        .as_ref()
                        .and_then(|state| state.session_id.clone())
                })
                .unwrap_or_default();
            self.builder = self.builder.executor(Arc::new(SessionStoreExecutor::new(
                store.clone(),
                session_id.clone(),
            )));
            let mut durability = AgentDurability::new(session_id, store);
            if let Some(stream_archive) = self.stream_archive.clone() {
                durability = durability.stream_archive(stream_archive);
            }
            if let Some(replay_event_log) = self.replay_event_log.clone() {
                durability = durability.replay_event_log(replay_event_log);
            }
            Some(durability)
        } else {
            None
        };
        let app = self.builder.build_app();
        let mut session = match (self.state, self.context) {
            (Some(state), _) => app.session_from_state(state),
            (None, Some(context)) => app.session_with_context(context),
            (None, None) => app.session(),
        };
        if let Some(durability) = durability.as_ref() {
            session
                .context_mut()
                .set_session_id(durability.session_id.clone());
        }
        if let Some(security) = self.security {
            session.context_mut().security = security;
        }
        if let Some(environment) = self.environment {
            session.set_environment(environment);
        }
        AgentRuntime {
            app,
            session,
            durability,
        }
    }
}

/// Owned SDK runtime containing app protocols and the active session.
pub struct AgentRuntime {
    app: AgentApp,
    session: AgentSession,
    durability: Option<AgentDurability>,
}

impl AgentRuntime {
    /// Return the underlying application facade.
    #[must_use]
    pub const fn app(&self) -> &AgentApp {
        &self.app
    }

    /// Return the active session.
    #[must_use]
    pub const fn session(&self) -> &AgentSession {
        &self.session
    }

    /// Return the mutable active session.
    pub const fn session_mut(&mut self) -> &mut AgentSession {
        &mut self.session
    }

    /// Consume the runtime into its session.
    #[must_use]
    pub fn into_session(self) -> AgentSession {
        self.session
    }

    /// Return the durable session id when this runtime is store-backed.
    #[must_use]
    pub fn durable_session_id(&self) -> Option<&SessionId> {
        self.durability
            .as_ref()
            .map(|durability| &durability.session_id)
    }

    /// Return the bound durable session store when configured.
    #[must_use]
    pub fn session_store(&self) -> Option<&Arc<dyn SessionStore>> {
        self.durability
            .as_ref()
            .map(|durability| &durability.session_store)
    }

    /// Export curated portable session state.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        self.session.export_state()
    }

    /// Export full Starweaver session state.
    #[must_use]
    pub fn export_full_state(&self) -> ResumableState {
        self.session.export_full_state()
    }

    /// Export the active environment provider state.
    ///
    /// # Errors
    ///
    /// Returns provider errors from the active environment.
    pub async fn export_environment_state(
        &self,
    ) -> Result<Option<EnvironmentState>, EnvironmentError> {
        self.session.export_environment_state().await
    }

    /// Restore the active environment provider.
    pub fn restore_environment(&mut self, provider: DynEnvironmentProvider) {
        self.session.restore_environment(provider);
    }

    /// Restore the active environment provider from an exported provider state.
    ///
    /// # Errors
    ///
    /// Returns an error when no registered factory can restore the state.
    pub fn restore_environment_from_state(
        &mut self,
        factories: &EnvironmentProviderFactoryRegistry,
        state: &EnvironmentState,
    ) -> Result<(), EnvironmentError> {
        self.restore_environment(factories.restore(state)?);
        Ok(())
    }

    /// Restore the active environment provider after host-owned resources are restored.
    ///
    /// # Errors
    ///
    /// Returns an error when a resource factory fails or no registered provider
    /// factory can restore the resulting state.
    pub async fn restore_environment_from_state_with_resources(
        &mut self,
        factories: &EnvironmentProviderFactoryRegistry,
        resource_factories: &ResourceRestoreFactoryRegistry,
        state: &EnvironmentState,
    ) -> Result<(), EnvironmentError> {
        let mut state = state.clone();
        state.resources = resource_factories
            .restore_typed_all(&state.resources)
            .await?;
        self.restore_environment(factories.restore(&state)?);
        Ok(())
    }

    /// Run with the owned session context.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
    ) -> Result<AgentResult, AgentError> {
        let input = prompt.into();
        if self.durability.is_some() {
            self.ensure_durable_session().await?;
            let stream = self.session.run_stream(input.clone()).await?;
            self.persist_stream_result(&input, &stream, None).await?;
            Ok(stream.result)
        } else {
            self.session.run(input).await
        }
    }

    /// Run with per-run SDK overrides.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_with_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentResult, AgentError> {
        let input = prompt.into();
        if self.durability.is_some() {
            self.ensure_durable_session().await?;
            let stream = self
                .session
                .run_stream_with_options(input.clone(), options)
                .await?;
            self.persist_stream_result(&input, &stream, None).await?;
            Ok(stream.result)
        } else {
            self.session.run_with_options(input, options).await
        }
    }

    /// Run and collect typed stream records.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime run fails.
    pub async fn run_stream(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
    ) -> Result<AgentStreamResult, AgentError> {
        let input = prompt.into();
        self.ensure_durable_session().await?;
        let stream = self.session.run_stream(input.clone()).await?;
        self.persist_stream_result(&input, &stream, None).await?;
        Ok(stream)
    }

    /// Load a durable resume snapshot by session and run id.
    ///
    /// # Errors
    ///
    /// Returns an error when this runtime is not store-backed or the store cannot
    /// assemble the resume snapshot.
    pub async fn resume_snapshot(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
    ) -> Result<SessionResumeSnapshot, AgentDurabilityError> {
        let durability = self
            .durability
            .as_ref()
            .ok_or(AgentDurabilityError::MissingSessionStore)?;
        durability
            .session_store
            .resume_snapshot(session_id, run_id)
            .await
            .map_err(AgentDurabilityError::from)
    }

    /// Resolve HITL decisions for a durable waiting run and continue execution.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime is not store-backed, the waiting run
    /// cannot be loaded, HITL decisions are invalid, or resumed execution fails.
    pub async fn resume_after_hitl_by_id(
        &mut self,
        session_id: &SessionId,
        run_id: &RunId,
        results: AgentHitlResults,
    ) -> Result<AgentResult, AgentDurabilityError> {
        let durability = self
            .durability
            .clone()
            .ok_or(AgentDurabilityError::MissingSessionStore)?;
        let snapshot = durability
            .session_store
            .resume_snapshot(session_id, run_id)
            .await?;
        let checkpoint = snapshot.latest_checkpoint.clone().ok_or_else(|| {
            AgentDurabilityError::MissingCheckpointState {
                session_id: session_id.as_str().to_string(),
                run_id: run_id.as_str().to_string(),
            }
        })?;
        let mut session = self.app.session_from_state(snapshot.state.clone());
        session.context_mut().set_session_id(session_id.clone());
        if results.is_empty() {
            if session.context().pending_tool_returns.is_empty() {
                return Err(AgentHitlError::NoWaitingRun.into());
            }
        } else {
            session
                .inject_hitl_results_for_state(&checkpoint.state, results.clone())
                .await?;
            persist_hitl_decisions(&durability, &snapshot, &results).await?;
        }
        let input = starweaver_runtime::AgentInput::text("");
        let stream = session.run_stream(input.clone()).await?;
        self.session = session;
        self.persist_stream_result(&input, &stream, Some(run_id.clone()))
            .await?;
        durability
            .session_store
            .update_run_status(
                session_id,
                run_id,
                SessionRunStatus::Completed,
                Some(format!(
                    "resumed in {}",
                    stream.result.state.run_id.as_str()
                )),
            )
            .await?;
        Ok(stream.result)
    }

    /// Start a live stream run.
    #[must_use]
    pub fn stream(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
    ) -> AgentStreamHandle {
        self.session.stream(prompt)
    }

    /// Try to start a live stream run.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        self.session.try_stream(prompt)
    }

    /// Start a live stream run with explicit stream delivery options.
    #[must_use]
    pub fn stream_with_stream_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        stream_options: AgentStreamOptions,
    ) -> AgentStreamHandle {
        self.session
            .stream_with_stream_options(prompt, stream_options)
    }

    /// Try to start a live stream run with explicit stream delivery options.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_stream_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        stream_options: AgentStreamOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        self.session
            .try_stream_with_stream_options(prompt, stream_options)
    }

    /// Start a live stream run with per-run SDK overrides.
    #[must_use]
    pub fn stream_with_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        options: AgentRunOptions,
    ) -> AgentStreamHandle {
        self.session.stream_with_options(prompt, options)
    }

    /// Try to start a live stream run with per-run SDK overrides.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        options: AgentRunOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        self.session.try_stream_with_options(prompt, options)
    }

    /// Start a live stream run with both run overrides and stream delivery options.
    #[must_use]
    pub fn stream_with_run_and_stream_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        options: AgentRunOptions,
        stream_options: AgentStreamOptions,
    ) -> AgentStreamHandle {
        self.session
            .stream_with_run_and_stream_options(prompt, options, stream_options)
    }

    /// Try to start a live stream run with both run overrides and stream delivery options.
    ///
    /// # Errors
    ///
    /// Returns `AgentStreamError::RuntimeUnavailable` when called outside an
    /// active Tokio runtime.
    pub fn try_stream_with_run_and_stream_options(
        &mut self,
        prompt: impl Into<starweaver_runtime::AgentInput>,
        options: AgentRunOptions,
        stream_options: AgentStreamOptions,
    ) -> Result<AgentStreamHandle, AgentStreamError> {
        self.session
            .try_stream_with_run_and_stream_options(prompt, options, stream_options)
    }

    /// Wait for a live stream to finish and persist its durable records.
    ///
    /// The `input` value must match the prompt used to start the handle. It is
    /// stored as the durable run input because live handles intentionally stay
    /// transport-only and do not own persistence state.
    ///
    /// # Errors
    ///
    /// Returns stream errors from the live handle or durable persistence errors.
    pub async fn finish_stream(
        &mut self,
        input: impl Into<starweaver_runtime::AgentInput>,
        handle: AgentStreamHandle,
    ) -> Result<crate::AgentLiveStreamResult, AgentDurabilityError> {
        let input = input.into();
        self.ensure_durable_session().await?;
        let completion = handle.complete().await;
        if let Some(result) = completion.result {
            self.session.replace_context(result.context.clone());
            self.session.record_result(&result.result);
            let stream = result.into_stream_result();
            self.persist_stream_result(&input, &stream, None).await?;
            return Ok(crate::AgentLiveStreamResult {
                result: stream.result,
                context: self.session.context().clone(),
                events: stream.events,
            });
        }
        self.persist_stream_failure(&input, &completion).await?;
        Err(AgentDurabilityError::Stream(
            completion.error.unwrap_or_else(|| {
                AgentStreamError::Join("stream completed without result".into())
            }),
        ))
    }

    async fn ensure_durable_session(&self) -> Result<(), AgentError> {
        let Some(durability) = self.durability.as_ref() else {
            return Ok(());
        };
        let mut record = match durability
            .session_store
            .load_session(&durability.session_id)
            .await
        {
            Ok(record) => record,
            Err(SessionStoreError::NotFound(_)) => {
                SessionRecord::new(durability.session_id.clone())
            }
            Err(error) => return Err(agent_error_from_session_store(&error)),
        };
        record.state = self.session.export_full_state();
        record.trace_context = self.session.context().trace_context.clone();
        record.metadata.insert(
            "agent_id".to_string(),
            serde_json::json!(self.session.context().agent_id.as_str()),
        );
        if let Some(agent) = self
            .session
            .context()
            .agent_registry
            .get(self.session.context().agent_id.as_str())
        {
            record.metadata.insert(
                "agent_name".to_string(),
                serde_json::json!(agent.agent_name),
            );
        }
        durability
            .session_store
            .save_session(record)
            .await
            .map_err(|error| agent_error_from_session_store(&error))
    }

    async fn persist_stream_result(
        &self,
        input: &starweaver_runtime::AgentInput,
        stream: &AgentStreamResult,
        restore_from_run_id: Option<RunId>,
    ) -> Result<(), AgentError> {
        let Some(durability) = self.durability.clone() else {
            return Ok(());
        };
        let result = &stream.result;
        let mut run = RunRecord::new(
            durability.session_id.clone(),
            result.state.run_id.clone(),
            result.state.conversation_id.clone(),
        );
        run.input = input_parts_from_agent_input(input);
        run.status = session_run_status(result.state.status);
        run.output_preview = (!result.output.is_empty()).then(|| result.output.clone());
        run.structured_output = result
            .structured_output
            .clone()
            .unwrap_or(serde_json::Value::Null);
        run.trace_context = self.session.context().trace_context.clone();
        run.restore_from_run_id = restore_from_run_id;
        run.metadata.insert(
            "run_step".to_string(),
            serde_json::json!(result.state.run_step),
        );
        durability
            .session_store
            .append_run(run)
            .await
            .map_err(|error| agent_error_from_session_store(&error))?;
        durability
            .session_store
            .save_context_state(&durability.session_id, self.session.export_full_state())
            .await
            .map_err(|error| agent_error_from_session_store(&error))?;
        if !stream.events.is_empty() {
            durability
                .session_store
                .append_stream_records(
                    &durability.session_id,
                    &result.state.run_id,
                    stream.events.clone(),
                )
                .await
                .map_err(|error| agent_error_from_session_store(&error))?;
            persist_stream_archive_and_replay(
                &durability,
                self.session.context(),
                &result.state.run_id,
                result.state.status,
                &stream.events,
            )
            .await
            .map_err(|error| agent_error_from_replay(&error))?;
        }
        persist_pending_hitl_records(&durability, self.session.context(), &result.state)
            .await
            .map_err(|error| agent_error_from_session_store(&error))
    }

    async fn persist_stream_failure(
        &mut self,
        input: &starweaver_runtime::AgentInput,
        completion: &AgentStreamCompletion,
    ) -> Result<(), AgentDurabilityError> {
        let Some(durability) = self.durability.clone() else {
            return Ok(());
        };
        let mut context = AgentContext::from_state(completion.state.clone());
        context.set_session_id(durability.session_id.clone());
        self.session.replace_context(context);
        self.ensure_durable_session().await?;
        let Some(run_id) = completion.state.run_id.clone() else {
            return Ok(());
        };
        let conversation_id = completion.state.conversation_id.clone().unwrap_or_default();
        let fallback_error = AgentStreamError::Join("stream completed without result".to_string());
        let status =
            live_stream_error_run_status(completion.error.as_ref().unwrap_or(&fallback_error));
        let mut run = RunRecord::new(
            durability.session_id.clone(),
            run_id.clone(),
            conversation_id,
        );
        run.input = input_parts_from_agent_input(input);
        run.status = session_run_status(status);
        run.structured_output = serde_json::Value::Null;
        run.trace_context = self.session.context().trace_context.clone();
        if let Some(error) = completion.error.as_ref() {
            run.metadata.insert(
                "live_stream_error".to_string(),
                serde_json::json!(error.to_string()),
            );
        }
        durability.session_store.append_run(run).await?;
        durability
            .session_store
            .save_context_state(&durability.session_id, self.session.export_full_state())
            .await?;
        if !completion.events.is_empty() {
            durability
                .session_store
                .append_stream_records(&durability.session_id, &run_id, completion.events.clone())
                .await?;
        }
        persist_stream_archive_and_replay(
            &durability,
            self.session.context(),
            &run_id,
            status,
            &completion.events,
        )
        .await?;
        Ok(())
    }
}

fn agent_error_from_session_store(error: &SessionStoreError) -> AgentError {
    AgentError::Executor(AgentExecutorError::Failed(error.to_string()))
}

fn agent_error_from_replay(error: &starweaver_stream::ReplayError) -> AgentError {
    AgentError::Executor(AgentExecutorError::Failed(error.to_string()))
}

const fn session_run_status(status: RunStatus) -> SessionRunStatus {
    match status {
        RunStatus::Starting | RunStatus::Running => SessionRunStatus::Running,
        RunStatus::Waiting => SessionRunStatus::Waiting,
        RunStatus::Completed => SessionRunStatus::Completed,
        RunStatus::Failed => SessionRunStatus::Failed,
        RunStatus::Cancelled => SessionRunStatus::Cancelled,
    }
}

const fn live_stream_error_run_status(error: &AgentStreamError) -> RunStatus {
    match error {
        AgentStreamError::Interrupted => RunStatus::Cancelled,
        AgentStreamError::RuntimeUnavailable(_)
        | AgentStreamError::Join(_)
        | AgentStreamError::Agent(_) => RunStatus::Failed,
    }
}

fn input_parts_from_agent_input(input: &starweaver_runtime::AgentInput) -> Vec<InputPart> {
    input
        .content
        .iter()
        .map(|part| match part {
            ContentPart::Text { text } => InputPart::text(text.clone()),
            ContentPart::ImageUrl { url } | ContentPart::FileUrl { url, .. } => {
                InputPart::url(url.clone())
            }
            ContentPart::Binary { .. }
            | ContentPart::ResourceRef { .. }
            | ContentPart::DataUrl { .. } => InputPart::Mode {
                mode: "content_part".to_string(),
                config: serde_json::to_value(part).unwrap_or(serde_json::Value::Null),
                metadata: starweaver_core::Metadata::default(),
            },
        })
        .collect()
}

async fn persist_pending_hitl_records(
    durability: &AgentDurability,
    context: &AgentContext,
    state: &starweaver_runtime::AgentRunState,
) -> Result<(), SessionStoreError> {
    for tool_return in &state.pending_approval_tool_returns {
        let input = ToolReturnRecordInput::new(
            &durability.session_id,
            &state.run_id,
            &tool_return.tool_call_id,
            &tool_return.name,
            &tool_return.metadata,
        )
        .with_trace_context(&context.trace_context);
        if let Some(record) = starweaver_session::ApprovalRecord::from_tool_return(&input) {
            durability.session_store.append_approval(record).await?;
        }
    }
    for tool_return in &state.deferred_tool_returns {
        let input = ToolReturnRecordInput::new(
            &durability.session_id,
            &state.run_id,
            &tool_return.tool_call_id,
            &tool_return.name,
            &tool_return.metadata,
        )
        .with_trace_context(&context.trace_context);
        if let Some(record) = starweaver_session::DeferredToolRecord::from_tool_return(&input) {
            durability
                .session_store
                .append_deferred_tool(record)
                .await?;
        }
    }
    Ok(())
}

async fn persist_hitl_decisions(
    durability: &AgentDurability,
    snapshot: &SessionResumeSnapshot,
    results: &AgentHitlResults,
) -> Result<(), AgentDurabilityError> {
    let now = Utc::now();
    for mut record in snapshot.approvals.clone() {
        let Some(decision) = results
            .approvals
            .get(&record.action_id)
            .or_else(|| results.approvals.get(&record.approval_id))
        else {
            continue;
        };
        let approval_decision = decision.clone().into_approval_decision();
        record.status = approval_decision.status;
        record.decision = Some(approval_decision);
        record.updated_at = now;
        durability.session_store.append_approval(record).await?;
    }
    for mut record in snapshot.deferred_tools.clone() {
        let Some(result) = results
            .deferred_results
            .results
            .iter()
            .find(|result| result.deferred_id == record.deferred_id)
        else {
            continue;
        };
        record.status = result.status;
        record.response = result.response.clone();
        record.metadata.extend(result.metadata.clone());
        record.updated_at = now;
        durability
            .session_store
            .append_deferred_tool(record)
            .await?;
    }
    Ok(())
}

async fn persist_stream_archive_and_replay(
    durability: &AgentDurability,
    context: &AgentContext,
    run_id: &RunId,
    status: RunStatus,
    records: &[AgentStreamRecord],
) -> Result<(), starweaver_stream::ReplayError> {
    if let Some(archive) = durability.stream_archive.as_ref() {
        archive
            .append_raw_records(&durability.session_id, run_id, records.to_vec())
            .await?;
    }
    if durability.stream_archive.is_none() && durability.replay_event_log.is_none() {
        return Ok(());
    }
    let scope = ReplayScope::run(run_id.as_str());
    let mut projection_context =
        DisplayProjectionContext::new(durability.session_id.clone(), run_id.clone());
    projection_context.agent_id = Some(context.agent_id.clone());
    if let Some(agent) = context.agent_registry.get(context.agent_id.as_str()) {
        projection_context.agent_name = Some(agent.agent_name.clone());
    }
    projection_context.trace_context = context.trace_context.clone();
    let mut display_messages = Vec::new();
    for record in records {
        display_messages.extend(
            durability
                .projector
                .project(&projection_context, record)
                .await,
        );
    }
    resequence_display_messages(&mut display_messages);
    if let Some(archive) = durability.stream_archive.as_ref() {
        archive
            .append_display_messages(scope.clone(), display_messages.clone())
            .await?;
    }
    if let Some(log) = durability.replay_event_log.as_ref() {
        for message in display_messages {
            log.append(scope.clone(), ReplayEvent::display(scope.clone(), message))
                .await?;
        }
        if let Some(marker) = terminal_marker(status) {
            let sequence = records
                .last()
                .map_or(0, |record| record.sequence.saturating_add(1));
            log.append(
                scope.clone(),
                ReplayEvent::new(scope, sequence, ReplayEventKind::Terminal { marker }),
            )
            .await?;
        }
    }
    Ok(())
}

fn resequence_display_messages(messages: &mut [starweaver_stream::DisplayMessage]) {
    for (sequence, message) in messages.iter_mut().enumerate() {
        message.sequence = sequence;
    }
}

fn terminal_marker(status: RunStatus) -> Option<StreamTerminalMarker> {
    match status {
        RunStatus::Completed => Some(StreamTerminalMarker::RunCompleted),
        RunStatus::Failed => Some(StreamTerminalMarker::RunFailed {
            code: "agent_failed".to_string(),
            message: "agent run failed".to_string(),
        }),
        RunStatus::Cancelled => Some(StreamTerminalMarker::RunCancelled {
            reason: "agent run cancelled".to_string(),
        }),
        RunStatus::Starting | RunStatus::Running | RunStatus::Waiting => None,
    }
}

/// Create an owned runtime builder from a model adapter.
#[must_use]
pub fn agent_runtime(model: Arc<dyn ModelAdapter>) -> AgentRuntimeBuilder {
    AgentRuntimeBuilder::new(model)
}
