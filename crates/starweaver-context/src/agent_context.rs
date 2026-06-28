use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{AgentId, ConversationId, Metadata, RunId, SessionId, TraceContext};
use starweaver_model::{
    ContentPart, ModelMessage, ModelRequest, ModelRequestPart, ModelResponsePart, ToolReturnPart,
};
use starweaver_usage::{
    add_optional_pricing, PricingEstimate, Usage, UsageAgentTotal, UsageSnapshot,
    UsageSnapshotEntry,
};

use crate::{
    runtime_context, AgentEvent, AgentInfo, AgentStreamQueueRegistry, BusMessage,
    ContextLifecycleState, DependencyStore, EventBus, MessageBus, ModelConfig, NoteStore,
    ResumableExportOptions, ResumableState, SecurityConfig, StateStore, Task, TaskManager,
    TaskSnapshot, ToolConfig, ToolIdWrapper, ToolSearchInvalidation, ToolSearchState,
    WrapperMetadata, TASK_SNAPSHOT_EVENT_KIND,
};

/// Lifecycle-wide agent context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentContext {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Stable logical session affinity identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    /// Parent run identifier if this context belongs to a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
    /// Tool returns to inject at the start of the next run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_tool_returns: Vec<ToolReturnPart>,
    /// Subagent message history keyed by agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub subagent_history: BTreeMap<String, Vec<ModelMessage>>,
    /// User prompt content collected for the current run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_prompts: Option<Vec<ContentPart>>,
    /// Visible assistant response immediately before the current user prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_assistant_response_reference: Option<String>,
    /// Accumulated user steering messages for compact restore.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering_messages: Vec<String>,
    /// Rendered handoff message for post-compact or post-handoff restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_message: Option<String>,
    /// Force environment/runtime context injection on the next filter pass.
    #[serde(default)]
    pub force_inject_context: bool,
    /// Extra environment variables for shell command execution.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub shell_env: BTreeMap<String, String>,
    /// Metadata for deferred tool calls.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub deferred_tool_metadata: BTreeMap<String, Metadata>,
    /// Agent registry keyed by agent id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agent_registry: BTreeMap<String, AgentInfo>,
    /// Tool names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_required_tools: Vec<String>,
    /// MCP server names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_required_mcp_servers: Vec<String>,
    /// Files to auto-load on next request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_load_files: Vec<String>,
    /// Typed task manager.
    #[serde(default, skip_serializing_if = "TaskManager::is_empty")]
    pub task_manager: TaskManager,
    /// Tool names loaded via tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_search_loaded_tools: Vec<String>,
    /// Namespace IDs loaded via tool search.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_search_loaded_namespaces: Vec<String>,
    /// Context-injection tag names that should be stripped or refreshed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injected_context_tags: Vec<String>,
    /// Active context management tool names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_manage_tool_names: Vec<String>,
    /// Active tool capability tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_tags: Vec<String>,
    /// Tool call ID wrapper for provider-normalized tool IDs.
    #[serde(default, skip_serializing_if = "ToolIdWrapper::is_empty")]
    pub tool_id_wrapper: ToolIdWrapper,
    /// Runtime stream queue registry placeholder.
    #[serde(default, skip_serializing_if = "AgentStreamQueueRegistry::is_empty")]
    pub agent_stream_queues: AgentStreamQueueRegistry,
    /// Wrapper metadata carried by the context.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub wrapper_metadata: WrapperMetadata,
    /// Runtime lifecycle state.
    #[serde(default, skip_serializing_if = "ContextLifecycleState::is_default")]
    pub lifecycle: ContextLifecycleState,
    /// Accumulated usage.
    #[serde(default)]
    pub usage: Usage,
    /// Per-run cumulative usage ledger entries keyed by stable source id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub usage_snapshot_entries: BTreeMap<String, UsageSnapshotEntry>,
    /// Model/runtime configuration used for injected runtime context and tool policies.
    #[serde(default, skip_serializing_if = "ModelConfig::is_default")]
    pub model_config: ModelConfig,
    /// Tool-level configuration used by first-party and host tools.
    #[serde(default, skip_serializing_if = "ToolConfig::is_default")]
    pub tool_config: ToolConfig,
    /// Security-related runtime configuration.
    #[serde(default, skip_serializing_if = "SecurityConfig::is_default")]
    pub security: SecurityConfig,
    /// Context creation/entry time used for elapsed runtime context.
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
    /// Context exit time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// State store.
    #[serde(default)]
    pub state: StateStore,
    /// Event bus.
    #[serde(default)]
    pub events: EventBus,
    /// Persisted notes.
    #[serde(default, skip_serializing_if = "NoteStore::is_empty")]
    pub notes: NoteStore,
    /// Message bus.
    #[serde(default)]
    pub messages: MessageBus,
    /// Trace correlation context.
    #[serde(default, skip_serializing_if = "TraceContext::is_empty")]
    pub trace_context: TraceContext,
    /// Context metadata.
    #[serde(default, skip_serializing_if = "Metadata::is_empty")]
    pub metadata: Metadata,
    /// Typed dependencies, skipped from serialization.
    #[serde(skip)]
    pub dependencies: DependencyStore,
}

impl AgentContext {
    /// Create a fresh context.
    #[must_use]
    pub fn new(agent_id: AgentId) -> Self {
        let mut agent_registry = BTreeMap::new();
        agent_registry.insert(
            agent_id.as_str().to_string(),
            AgentInfo::new(agent_id.as_str(), agent_id.as_str()),
        );
        let mut messages = MessageBus::new();
        messages.subscribe(agent_id.as_str());
        Self {
            agent_id,
            run_id: None,
            session_id: None,
            parent_run_id: None,
            conversation_id: ConversationId::new(),
            message_history: Vec::new(),
            pending_tool_returns: Vec::new(),
            subagent_history: BTreeMap::new(),
            user_prompts: None,
            previous_assistant_response_reference: None,
            steering_messages: Vec::new(),
            handoff_message: None,
            force_inject_context: false,
            shell_env: BTreeMap::new(),
            deferred_tool_metadata: BTreeMap::new(),
            agent_registry,
            approval_required_tools: Vec::new(),
            approval_required_mcp_servers: Vec::new(),
            auto_load_files: Vec::new(),
            task_manager: TaskManager::new(),
            tool_search_loaded_tools: Vec::new(),
            tool_search_loaded_namespaces: Vec::new(),
            injected_context_tags: vec![
                "runtime-context".to_string(),
                "environment-context".to_string(),
            ],
            context_manage_tool_names: Vec::new(),
            tool_tags: Vec::new(),
            tool_id_wrapper: ToolIdWrapper::default(),
            agent_stream_queues: AgentStreamQueueRegistry::default(),
            wrapper_metadata: Metadata::default(),
            lifecycle: ContextLifecycleState::default(),
            usage: Usage::default(),
            usage_snapshot_entries: BTreeMap::new(),
            model_config: ModelConfig::default(),
            tool_config: ToolConfig::default(),
            security: SecurityConfig::default(),
            started_at: Utc::now(),
            ended_at: None,
            state: StateStore::new(),
            events: EventBus::new(),
            notes: NoteStore::new(),
            messages,
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
            dependencies: DependencyStore::new(),
        }
    }

    /// Restore a context from serialized state.
    #[must_use]
    pub fn from_state(state: ResumableState) -> Self {
        let mut context = Self::new(state.agent_id.clone());
        context.run_id = state.run_id;
        context.session_id = state.session_id;
        context.conversation_id = state.conversation_id.unwrap_or_default();
        context.message_history = state.message_history;
        context.pending_tool_returns = state.pending_tool_returns;
        context.subagent_history = state.subagent_history;
        context.user_prompts = state.user_prompts;
        context.previous_assistant_response_reference = state.previous_assistant_response_reference;
        context.steering_messages = state.steering_messages;
        context.handoff_message = state.handoff_message;
        context.shell_env = state.shell_env;
        context.deferred_tool_metadata = state.deferred_tool_metadata;
        context.agent_registry = state.agent_registry;
        if context.agent_registry.is_empty() {
            context.agent_registry.insert(
                context.agent_id.as_str().to_string(),
                AgentInfo::new(context.agent_id.as_str(), context.agent_id.as_str()),
            );
        }
        context.approval_required_tools = state.approval_required_tools;
        context.approval_required_mcp_servers = state.approval_required_mcp_servers;
        context.security = state.security;
        context.auto_load_files = state.auto_load_files;
        context.task_manager = TaskManager::from_exported(state.tasks);
        context.notes = NoteStore::from_map(state.notes);
        context.tool_search_loaded_tools = state.tool_search_loaded_tools;
        context.tool_search_loaded_namespaces = state.tool_search_loaded_namespaces;
        context.usage = state.usage;
        context.usage_snapshot_entries = state.usage_snapshot_entries;
        context.model_config = state.model_config;
        context.tool_config = state.tool_config;
        context.started_at = state.started_at;
        context.state = state.state;
        context.messages = state.message_bus;
        context.trace_context = state.trace_snapshot;
        context.metadata = state.metadata;
        context
    }

    /// Export curated portable context state for session restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        self.export_state_with_options(ResumableExportOptions::curated())
    }

    /// Export full Starweaver runtime context state.
    #[must_use]
    pub fn export_full_state(&self) -> ResumableState {
        self.export_state_with_options(ResumableExportOptions::full())
    }

    /// Export context state with explicit export options.
    #[must_use]
    pub fn export_state_with_options(&self, options: ResumableExportOptions) -> ResumableState {
        ResumableState {
            agent_id: self.agent_id.clone(),
            run_id: options
                .include_starweaver_extensions()
                .then(|| self.run_id.clone())
                .flatten(),
            session_id: self.session_id.clone(),
            conversation_id: options
                .include_starweaver_extensions()
                .then(|| self.conversation_id.clone()),
            message_history: if options.include_starweaver_extensions() {
                self.message_history.clone()
            } else {
                Vec::new()
            },
            pending_tool_returns: if options.include_starweaver_extensions() {
                self.pending_tool_returns.clone()
            } else {
                Vec::new()
            },
            subagent_history: if options.include_subagent() {
                self.subagent_history.clone()
            } else {
                BTreeMap::new()
            },
            user_prompts: self.user_prompts.clone(),
            previous_assistant_response_reference: self
                .previous_assistant_response_reference
                .clone(),
            steering_messages: self.steering_messages.clone(),
            handoff_message: self.handoff_message.clone(),
            shell_env: self.shell_env.clone(),
            deferred_tool_metadata: self.deferred_tool_metadata.clone(),
            agent_registry: if options.include_subagent() {
                self.agent_registry.clone()
            } else {
                BTreeMap::new()
            },
            approval_required_tools: self.approval_required_tools.clone(),
            approval_required_mcp_servers: self.approval_required_mcp_servers.clone(),
            security: if options.include_runtime_policy() {
                self.security.clone()
            } else {
                SecurityConfig::default()
            },
            auto_load_files: self.auto_load_files.clone(),
            tasks: self.task_manager.export_tasks(),
            notes: self.notes.to_map(),
            tool_search_loaded_tools: self.tool_search_loaded_tools.clone(),
            tool_search_loaded_namespaces: self.tool_search_loaded_namespaces.clone(),
            usage: if options.include_starweaver_extensions() {
                self.usage.clone()
            } else {
                Usage::default()
            },
            usage_snapshot_entries: if options.include_usage_ledger() {
                self.usage_snapshot_entries.clone()
            } else {
                BTreeMap::new()
            },
            model_config: if options.include_runtime_policy() {
                self.model_config.clone()
            } else {
                ModelConfig::default()
            },
            tool_config: if options.include_runtime_policy() {
                self.tool_config.clone()
            } else {
                ToolConfig::default()
            },
            started_at: if options.include_starweaver_extensions() {
                self.started_at
            } else {
                DateTime::<Utc>::UNIX_EPOCH
            },
            state: if options.include_starweaver_extensions() {
                self.state.clone()
            } else {
                StateStore::new()
            },
            message_bus: if options.include_starweaver_extensions() {
                self.messages.clone()
            } else {
                MessageBus::new()
            },
            trace_snapshot: if options.include_starweaver_extensions() {
                self.trace_context.clone()
            } else {
                TraceContext::default()
            },
            metadata: if options.include_starweaver_extensions() {
                self.metadata.clone()
            } else {
                Metadata::default()
            },
            extra: BTreeMap::new(),
        }
    }

    /// Replace context with serialized state.
    pub fn restore_state(&mut self, state: ResumableState) {
        let dependencies = self.dependencies.clone();
        let security = self.security.clone();
        *self = Self::from_state(state);
        self.dependencies = dependencies;
        // Current runtime security wins unless caller explicitly constructs a context from state
        // with `from_state`.
        self.security = security;
    }

    /// Set the stable logical session affinity identifier.
    pub fn set_session_id(&mut self, session_id: SessionId) {
        self.session_id = Some(session_id);
    }

    /// Return the stable logical session affinity identifier.
    #[must_use]
    pub const fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    /// Prepare context for a new run.
    pub fn prepare_new_run(&mut self) {
        self.run_id = Some(RunId::new());
        self.started_at = Utc::now();
        self.ended_at = None;
        self.lifecycle.entered = true;
        self.lifecycle.stream_queue_enabled = false;
        self.lifecycle.compact_depth = 0;
        self.tool_id_wrapper.clear();
        self.agent_stream_queues = AgentStreamQueueRegistry::default();
        if self.parent_run_id.is_none() && !self.metadata.contains_key("parent_agent_id") {
            self.usage_snapshot_entries.clear();
        }
        self.deferred_tool_metadata.clear();
        self.force_inject_context = false;
        self.previous_assistant_response_reference = None;
        self.messages.subscribe(self.agent_id.as_str());
    }

    /// Mark the active run as finished.
    pub fn finish_run(&mut self) {
        self.ended_at = Some(Utc::now());
        self.lifecycle.entered = false;
    }

    /// Create a child context for subagent execution using the same value for id and name.
    #[must_use]
    pub fn subagent_context(&self, agent_id: impl Into<String>) -> Self {
        let agent_id = agent_id.into();
        self.subagent_context_with_agent_id(agent_id.clone(), agent_id)
    }

    /// Create a child context for subagent execution with separate display name and stable id.
    #[must_use]
    pub fn subagent_context_with_agent_id(
        &self,
        agent_name: impl Into<String>,
        agent_id: impl Into<String>,
    ) -> Self {
        let agent_name = agent_name.into();
        let agent_id = AgentId::from_string(agent_id);
        let mut child = self.clone();
        child.agent_id = agent_id.clone();
        child.run_id = None;
        child.parent_run_id.clone_from(&self.run_id);
        child.message_history = self
            .subagent_history
            .get(agent_id.as_str())
            .cloned()
            .unwrap_or_default();
        child.user_prompts = None;
        child.previous_assistant_response_reference = None;
        child.steering_messages = Vec::new();
        child.handoff_message = None;
        child.tool_id_wrapper = ToolIdWrapper::default();
        child.tool_tags = Vec::new();
        child.started_at = Utc::now();
        child.ended_at = None;
        child.security = self.security.clone();
        child.metadata.insert(
            "parent_agent_id".to_string(),
            serde_json::json!(self.agent_id.as_str()),
        );
        child.metadata.insert(
            "agent_name".to_string(),
            serde_json::json!(agent_name.as_str()),
        );
        if let Some(run_id) = &self.run_id {
            child.metadata.insert(
                "parent_run_id".to_string(),
                serde_json::json!(run_id.as_str()),
            );
        }
        child.agent_registry.insert(
            agent_id.as_str().to_string(),
            AgentInfo::new(agent_id.as_str(), agent_name)
                .with_parent_agent_id(self.agent_id.as_str()),
        );
        child.messages.subscribe(agent_id.as_str());
        child
    }

    /// Absorb child context state that should survive successful subagent execution.
    pub fn absorb_subagent_context(&mut self, child: &Self) {
        let event_cursor = self.events.len();
        self.usage = child.usage.clone();
        self.usage_snapshot_entries
            .clone_from(&child.usage_snapshot_entries);
        self.notes = child.notes.clone();
        self.task_manager = child.task_manager.clone();
        self.state = child.state.clone();
        self.messages = child.messages.clone();
        self.agent_registry = child.agent_registry.clone();
        self.subagent_history.insert(
            child.agent_id.as_str().to_string(),
            child.message_history.clone(),
        );
        for (agent_id, history) in &child.subagent_history {
            self.subagent_history
                .entry(agent_id.clone())
                .or_insert_with(|| history.clone());
        }
        for event in child.events.events().iter().skip(event_cursor) {
            self.events.publish(event.clone());
        }
    }

    /// Attach trace correlation context.
    #[must_use]
    pub fn with_trace_context(mut self, trace_context: TraceContext) -> Self {
        self.trace_context = trace_context;
        self
    }

    /// Replace trace correlation context.
    pub fn set_trace_context(&mut self, trace_context: TraceContext) {
        self.trace_context = trace_context;
    }

    /// Record a tool name loaded through dynamic tool search.
    pub fn record_tool_search_loaded_tool(&mut self, tool_name: impl Into<String>) {
        push_unique(&mut self.tool_search_loaded_tools, tool_name.into());
    }

    /// Record a namespace loaded through dynamic tool search.
    pub fn record_tool_search_loaded_namespace(&mut self, namespace: impl Into<String>) {
        push_unique(&mut self.tool_search_loaded_namespaces, namespace.into());
    }

    /// Record loaded tool-search state in one update.
    pub fn record_tool_search_loaded(
        &mut self,
        tools: impl IntoIterator<Item = impl Into<String>>,
        namespaces: impl IntoIterator<Item = impl Into<String>>,
    ) {
        for tool in tools {
            self.record_tool_search_loaded_tool(tool);
        }
        for namespace in namespaces {
            self.record_tool_search_loaded_namespace(namespace);
        }
    }

    /// Clear all loaded tool-search state and return the removed values.
    pub fn clear_tool_search_loaded(&mut self) -> ToolSearchInvalidation {
        ToolSearchInvalidation {
            removed_tools: std::mem::take(&mut self.tool_search_loaded_tools),
            removed_namespaces: std::mem::take(&mut self.tool_search_loaded_namespaces),
        }
    }

    /// Retain only loaded tool-search entries accepted by the supplied predicates.
    pub fn retain_tool_search_loaded(
        &mut self,
        mut keep_tool: impl FnMut(&str) -> bool,
        mut keep_namespace: impl FnMut(&str) -> bool,
    ) -> ToolSearchInvalidation {
        ToolSearchInvalidation {
            removed_tools: retain_matching(&mut self.tool_search_loaded_tools, |tool| {
                keep_tool(tool)
            }),
            removed_namespaces: retain_matching(
                &mut self.tool_search_loaded_namespaces,
                |namespace| keep_namespace(namespace),
            ),
        }
    }

    /// Return the current tool-search state snapshot.
    #[must_use]
    pub fn tool_search_state(&self) -> ToolSearchState {
        ToolSearchState {
            loaded_tools: self.tool_search_loaded_tools.clone(),
            loaded_namespaces: self.tool_search_loaded_namespaces.clone(),
        }
    }

    /// Record a model message in context history.
    pub fn push_message(&mut self, message: ModelMessage) {
        self.message_history.push(message);
    }

    /// Record usage in the context ledger.
    pub const fn add_usage(&mut self, usage: &Usage) {
        self.usage.add_assign(usage);
    }

    /// Update one cumulative usage snapshot ledger entry and return the latest run snapshot.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn update_usage_snapshot_entry(
        &mut self,
        agent_id: impl Into<String>,
        agent_name: impl Into<String>,
        model_id: impl Into<String>,
        usage: Usage,
        estimate_pricing: Option<PricingEstimate>,
        usage_id: Option<String>,
        source: impl Into<String>,
        ledger_key: Option<String>,
    ) -> UsageSnapshot {
        let agent_id = agent_id.into();
        let entry = UsageSnapshotEntry {
            agent_id: agent_id.clone(),
            agent_name: agent_name.into(),
            model_id: model_id.into(),
            usage,
            estimate_pricing,
            usage_id,
            source: source.into(),
        };
        self.usage_snapshot_entries
            .insert(ledger_key.unwrap_or(agent_id), entry);
        self.build_usage_snapshot()
    }

    /// Update one cumulative external usage ledger entry.
    ///
    /// This helper is for host services, sub-systems, and adapters that need to
    /// include non-model usage in the run snapshot without pretending the usage
    /// came from the active model request.
    #[must_use]
    pub fn update_external_usage_snapshot_entry(
        &mut self,
        source_id: impl Into<String>,
        source_name: impl Into<String>,
        model_id: impl Into<String>,
        usage: Usage,
        estimate_pricing: Option<PricingEstimate>,
        usage_id: Option<String>,
    ) -> UsageSnapshot {
        let source_id = source_id.into();
        let ledger_key = usage_id.as_ref().map_or_else(
            || format!("external:{source_id}"),
            |usage_id| format!("external:{source_id}:{usage_id}"),
        );
        self.update_usage_snapshot_entry(
            source_id,
            source_name,
            model_id,
            usage,
            estimate_pricing,
            usage_id,
            "external",
            Some(ledger_key),
        )
    }

    /// Build a cumulative usage snapshot for this run.
    #[must_use]
    pub fn build_usage_snapshot(&self) -> UsageSnapshot {
        let mut total_usage = Usage::default();
        let mut estimate_pricing = None;
        let mut agent_usages = BTreeMap::<String, UsageAgentTotal>::new();
        let mut model_usages = BTreeMap::<String, Usage>::new();
        let mut model_estimate_pricing = BTreeMap::<String, PricingEstimate>::new();
        let mut entries = self
            .usage_snapshot_entries
            .values()
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        for entry in &entries {
            total_usage.add_assign(&entry.usage);
            add_optional_pricing(&mut estimate_pricing, entry.estimate_pricing.as_ref());
            if let Some(pricing) = &entry.estimate_pricing {
                model_estimate_pricing
                    .entry(entry.model_id.clone())
                    .or_default()
                    .add_assign(pricing);
            }
            if let Some(agent_total) = agent_usages.get_mut(&entry.agent_id) {
                agent_total.usage.add_assign(&entry.usage);
                if agent_total.model_id != entry.model_id {
                    agent_total.model_id = "multiple".to_string();
                }
                if agent_total.usage_id != entry.usage_id {
                    agent_total.usage_id = None;
                }
                add_optional_pricing(
                    &mut agent_total.estimate_pricing,
                    entry.estimate_pricing.as_ref(),
                );
            } else {
                agent_usages.insert(
                    entry.agent_id.clone(),
                    UsageAgentTotal {
                        agent_name: entry.agent_name.clone(),
                        model_id: entry.model_id.clone(),
                        usage: entry.usage.clone(),
                        estimate_pricing: entry.estimate_pricing.clone(),
                        usage_id: entry.usage_id.clone(),
                        source: entry.source.clone(),
                    },
                );
            }
            model_usages
                .entry(entry.model_id.clone())
                .or_default()
                .add_assign(&entry.usage);
        }
        UsageSnapshot {
            run_id: self
                .parent_run_id
                .as_ref()
                .or(self.run_id.as_ref())
                .map_or_else(String::new, |run_id| run_id.as_str().to_string()),
            latest_usage: None,
            total_usage,
            estimate_pricing,
            entries,
            agent_usages,
            model_usages,
            model_estimate_pricing,
        }
    }

    /// Return the latest model request usage reported by the provider.
    #[must_use]
    pub fn latest_request_usage(&self) -> Option<&Usage> {
        self.message_history.iter().rev().find_map(|message| {
            let ModelMessage::Response(response) = message else {
                return None;
            };
            (!response.usage.is_empty()).then_some(&response.usage)
        })
    }

    /// Return the latest model request token usage reported by the provider.
    #[must_use]
    pub fn latest_request_total_tokens(&self) -> Option<u64> {
        self.latest_request_usage()
            .and_then(|usage| (usage.total_tokens > 0).then_some(usage.total_tokens))
    }

    /// Append synthetic error tool returns for any unclosed tool calls in message history.
    ///
    /// This is used when a run fails or is interrupted after a provider has emitted tool calls but
    /// before every tool return has been recorded. It keeps recovered history acceptable to
    /// providers that require every tool call to be closed by a matching tool return.
    pub fn repair_dangling_tool_calls(&mut self, reason: impl Into<String>) -> usize {
        let reason = reason.into();
        let mut pending = Vec::<(String, String)>::new();
        for message in &self.message_history {
            match message {
                ModelMessage::Response(response) => {
                    for part in &response.parts {
                        match part {
                            ModelResponsePart::ToolCall(call)
                            | ModelResponsePart::ProviderToolCall { call, .. } => {
                                pending.push((call.id.clone(), call.name.clone()));
                            }
                            ModelResponsePart::Text { .. }
                            | ModelResponsePart::ProviderText { .. }
                            | ModelResponsePart::Thinking { .. }
                            | ModelResponsePart::ProviderThinking { .. }
                            | ModelResponsePart::NativeToolCall { .. }
                            | ModelResponsePart::NativeToolReturn { .. }
                            | ModelResponsePart::File { .. }
                            | ModelResponsePart::Compaction { .. }
                            | ModelResponsePart::ProviderOpaque { .. } => {}
                        }
                    }
                }
                ModelMessage::Request(request) => {
                    for part in &request.parts {
                        if let ModelRequestPart::ToolReturn(tool_return) = part {
                            pending.retain(|(id, _)| id != &tool_return.tool_call_id);
                        }
                    }
                }
            }
        }
        if pending.is_empty() {
            return 0;
        }
        let repaired_count = pending.len();
        let mut parts = Vec::with_capacity(repaired_count);
        for (tool_call_id, name) in pending {
            let mut metadata = Metadata::default();
            metadata.insert(
                "starweaver.repaired_dangling_tool_call".to_string(),
                serde_json::json!(true),
            );
            metadata.insert("reason".to_string(), serde_json::json!(reason.clone()));
            parts.push(ModelRequestPart::ToolReturn(
                ToolReturnPart::new(
                    tool_call_id,
                    name,
                    serde_json::json!({
                        "error": "tool_call_interrupted",
                        "message": reason.clone(),
                    }),
                )
                .with_error(true)
                .with_metadata(metadata),
            ));
        }
        self.message_history
            .push(ModelMessage::Request(ModelRequest {
                parts,
                timestamp: Some(Utc::now()),
                instructions: None,
                run_id: self.run_id.clone(),
                conversation_id: Some(self.conversation_id.clone()),
                metadata: serde_json::json!({
                    "starweaver.repaired_dangling_tool_calls": true,
                })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            }));
        repaired_count
    }

    /// Publish an event.
    pub fn publish_event(&mut self, event: AgentEvent) {
        self.events.publish(event);
    }

    /// Return all tasks from the typed task manager.
    #[must_use]
    pub fn tasks(&self) -> Vec<Task> {
        self.task_manager.list_all()
    }

    /// Replace all tasks in the typed task manager.
    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        self.task_manager.replace_all(tasks);
    }

    /// Return a full task snapshot.
    #[must_use]
    pub fn task_snapshot(&self) -> TaskSnapshot {
        TaskSnapshot {
            tasks: self.tasks(),
        }
    }

    /// Publish a full task snapshot event.
    pub fn publish_task_snapshot_event(&mut self) {
        self.publish_event(AgentEvent::new(
            TASK_SNAPSHOT_EVENT_KIND,
            self.task_snapshot().into_payload(),
        ));
    }

    /// Enqueue a message.
    pub fn enqueue_message(&mut self, message: BusMessage) {
        self.messages.send(message);
    }

    /// Send a bus message idempotently.
    pub fn send_message(&mut self, message: BusMessage) -> BusMessage {
        self.messages.send(message)
    }

    /// Consume unread bus messages for this context agent.
    pub fn consume_messages(&mut self) -> Vec<BusMessage> {
        self.messages.consume(self.agent_id.as_str())
    }

    /// Consume unread bus messages for a specific agent id.
    pub fn consume_messages_for(&mut self, agent_id: &str) -> Vec<BusMessage> {
        self.messages.consume(agent_id)
    }

    /// Consume unread bus messages matching a predicate for this context agent.
    pub fn consume_messages_matching(
        &mut self,
        predicate: impl Fn(&BusMessage) -> bool,
    ) -> Vec<BusMessage> {
        self.messages
            .consume_matching(self.agent_id.as_str(), predicate)
    }

    /// Subscribe the current agent to the message bus.
    pub fn subscribe_messages(&mut self) {
        self.messages.subscribe(self.agent_id.as_str());
    }

    /// Return wrapper metadata with built-in context fields and user overrides.
    #[must_use]
    pub fn get_wrapper_metadata(&self) -> Metadata {
        let mut metadata = Metadata::default();
        if let Some(run_id) = &self.run_id {
            metadata.insert("run_id".to_string(), serde_json::json!(run_id.as_str()));
        }
        if let Some(parent_run_id) = &self.parent_run_id {
            metadata.insert(
                "parent_run_id".to_string(),
                serde_json::json!(parent_run_id.as_str()),
            );
        }
        metadata.insert(
            "agent_id".to_string(),
            serde_json::json!(self.agent_id.as_str()),
        );
        for (key, value) in &self.wrapper_metadata {
            metadata.insert(key.clone(), value.clone());
        }
        metadata
    }

    /// Insert a typed dependency.
    pub fn insert_dependency<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert(value);
    }

    /// Insert a named typed dependency.
    pub fn insert_named_dependency<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.insert_named(name, value);
    }

    /// Get a typed dependency.
    #[must_use]
    pub fn dependency<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get::<T>()
    }

    /// Get a named typed dependency.
    #[must_use]
    pub fn named_dependency<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.dependencies.get_named::<T>(name)
    }

    /// Set the context window exposed in model-facing runtime context.
    pub const fn set_context_window(&mut self, context_window: Option<u64>) {
        self.model_config.context_window = context_window;
    }

    /// Merge runtime model defaults into this context.
    pub fn merge_model_config(&mut self, model_config: ModelConfig) {
        self.model_config.merge_from(model_config);
    }

    /// Replace the tool config for this context.
    pub fn set_tool_config(&mut self, mut tool_config: ToolConfig) {
        tool_config.normalize();
        self.tool_config = tool_config;
    }

    /// Merge runtime tool defaults into this context.
    pub fn merge_tool_config(&mut self, mut tool_config: ToolConfig) {
        tool_config.normalize();
        let existing_dynamic_patterns = self.tool_config.view_relaxed_text_dynamic_patterns.clone();
        for (source, patterns) in existing_dynamic_patterns {
            tool_config
                .view_relaxed_text_dynamic_patterns
                .entry(source)
                .or_insert(patterns);
        }
        self.tool_config = tool_config;
    }

    /// Render runtime context for model-facing requests.
    #[must_use]
    pub fn render_runtime_context(&self, is_user_prompt: bool) -> Option<String> {
        runtime_context::render_runtime_context(self, is_user_prompt)
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !value.is_empty() && !values.contains(&value) {
        values.push(value);
    }
}

fn retain_matching(values: &mut Vec<String>, mut keep: impl FnMut(&str) -> bool) -> Vec<String> {
    let mut retained = Vec::with_capacity(values.len());
    let mut removed = Vec::new();
    for value in std::mem::take(values) {
        if keep(&value) {
            retained.push(value);
        } else {
            removed.push(value);
        }
    }
    *values = retained;
    removed
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new(AgentId::default())
    }
}
