use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{
    AgentId, ConversationId, Metadata, RunId, TraceContext, Usage, UsageAgentTotal, UsageSnapshot,
    UsageSnapshotEntry,
};
use starweaver_model::{ContentPart, ModelMessage};

use crate::{
    runtime_context, task::TASK_STATE_DOMAIN, AgentEvent, AgentInfo, AgentStreamQueueRegistry,
    BusMessage, ContextLifecycleState, DependencyStore, EventBus, MessageBus, ModelConfig,
    NoteStore, ResumableExportOptions, ResumableState, SecurityConfig, StateStore, Task,
    TaskManager, TaskSnapshot, ToolConfig, ToolIdWrapper, WrapperMetadata,
    TASK_SNAPSHOT_EVENT_KIND,
};

/// Lifecycle-wide agent context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentContext {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Parent run identifier if this context belongs to a subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<RunId>,
    /// Provider-facing session identifier for model request headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    /// Provider-facing thread identifier for model request headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_thread_id: Option<String>,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
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
    /// Force environment/runtime instruction injection on the next filter pass.
    #[serde(default)]
    pub force_inject_instructions: bool,
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
    pub need_user_approve_tools: Vec<String>,
    /// MCP server names requiring approval.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub need_user_approve_mcps: Vec<String>,
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
            parent_run_id: None,
            provider_session_id: None,
            provider_thread_id: None,
            conversation_id: ConversationId::new(),
            message_history: Vec::new(),
            subagent_history: BTreeMap::new(),
            user_prompts: None,
            previous_assistant_response_reference: None,
            steering_messages: Vec::new(),
            handoff_message: None,
            force_inject_instructions: false,
            shell_env: BTreeMap::new(),
            deferred_tool_metadata: BTreeMap::new(),
            agent_registry,
            need_user_approve_tools: Vec::new(),
            need_user_approve_mcps: Vec::new(),
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
        context.conversation_id = state.conversation_id.unwrap_or_default();
        context.message_history = state.message_history;
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
        context.need_user_approve_tools = state.need_user_approve_tools;
        context.need_user_approve_mcps = state.need_user_approve_mcps;
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
        context.import_legacy_tasks_from_state();
        context.messages = state.message_bus;
        context.trace_context = state.trace_snapshot;
        context.metadata = state.metadata;
        context
    }

    /// Export ya-mono-style curated context state for session restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        self.export_state_with_options(ResumableExportOptions::ya_mono_curated())
    }

    /// Export legacy Starweaver full context state.
    #[must_use]
    pub fn export_full_state(&self) -> ResumableState {
        self.export_state_with_options(ResumableExportOptions::starweaver_legacy())
    }

    /// Export context state with explicit parity options.
    #[must_use]
    pub fn export_state_with_options(&self, options: ResumableExportOptions) -> ResumableState {
        ResumableState {
            agent_id: self.agent_id.clone(),
            run_id: options
                .include_starweaver_extensions()
                .then(|| self.run_id.clone())
                .flatten(),
            conversation_id: options
                .include_starweaver_extensions()
                .then(|| self.conversation_id.clone()),
            message_history: if options.include_starweaver_extensions() {
                self.message_history.clone()
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
            need_user_approve_tools: self.need_user_approve_tools.clone(),
            need_user_approve_mcps: self.need_user_approve_mcps.clone(),
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
        // Match ya-mono restore semantics: current runtime security wins unless caller explicitly
        // constructs a context from state with `from_state`.
        self.security = security;
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
        self.force_inject_instructions = false;
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
    }

    fn import_legacy_tasks_from_state(&mut self) {
        if !self.task_manager.is_empty() {
            return;
        }
        let Some(value) = self.state.get(TASK_STATE_DOMAIN).cloned() else {
            return;
        };
        if let Ok(snapshot) = serde_json::from_value::<TaskSnapshot>(value) {
            self.task_manager = TaskManager::from_tasks(snapshot.tasks);
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

    /// Record a model message in context history.
    pub fn push_message(&mut self, message: ModelMessage) {
        self.message_history.push(message);
    }

    /// Record usage in the context ledger.
    pub fn add_usage(&mut self, usage: &Usage) {
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
            usage_id,
            source: source.into(),
        };
        self.usage_snapshot_entries
            .insert(ledger_key.unwrap_or(agent_id), entry);
        self.build_usage_snapshot()
    }

    /// Build a cumulative usage snapshot for this run.
    #[must_use]
    pub fn build_usage_snapshot(&self) -> UsageSnapshot {
        let mut total_usage = Usage::default();
        let mut agent_usages = BTreeMap::<String, UsageAgentTotal>::new();
        let mut model_usages = BTreeMap::<String, Usage>::new();
        let mut entries = self
            .usage_snapshot_entries
            .values()
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        for entry in &entries {
            total_usage.add_assign(&entry.usage);
            if let Some(agent_total) = agent_usages.get_mut(&entry.agent_id) {
                agent_total.usage.add_assign(&entry.usage);
                if agent_total.model_id != entry.model_id {
                    agent_total.model_id = "multiple".to_string();
                }
                if agent_total.usage_id != entry.usage_id {
                    agent_total.usage_id = None;
                }
            } else {
                agent_usages.insert(
                    entry.agent_id.clone(),
                    UsageAgentTotal {
                        agent_name: entry.agent_name.clone(),
                        model_id: entry.model_id.clone(),
                        usage: entry.usage.clone(),
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
            entries,
            agent_usages,
            model_usages,
        }
    }

    /// Return the latest model request token usage reported by the provider.
    #[must_use]
    pub fn latest_request_total_tokens(&self) -> Option<u64> {
        self.message_history.iter().rev().find_map(|message| {
            let ModelMessage::Response(response) = message else {
                return None;
            };
            (response.usage.total_tokens > 0).then_some(response.usage.total_tokens)
        })
    }

    /// Publish an event.
    pub fn publish_event(&mut self, event: AgentEvent) {
        self.events.publish(event);
    }

    /// Return all tasks from the typed task manager.
    #[must_use]
    pub fn tasks(&self) -> Vec<Task> {
        if !self.task_manager.is_empty() {
            return self.task_manager.list_all();
        }
        self.state
            .get(TASK_STATE_DOMAIN)
            .cloned()
            .and_then(|value| serde_json::from_value::<TaskSnapshot>(value).ok())
            .map_or_else(Vec::new, |snapshot| snapshot.tasks)
    }

    /// Replace all tasks in the typed task manager and legacy task state domain.
    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
        self.task_manager.replace_all(tasks.clone());
        self.state.set(
            TASK_STATE_DOMAIN,
            serde_json::to_value(TaskSnapshot { tasks })
                .unwrap_or_else(|_| serde_json::json!({"tasks": []})),
        );
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
        self.messages.enqueue(message);
    }

    /// Send a ya-mono-style bus message idempotently.
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

    /// Return stable provider headers for model construction.
    #[must_use]
    pub fn get_model_extra_headers(&self) -> BTreeMap<String, String> {
        let session_id = self
            .provider_session_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| self.run_id.as_ref().map(RunId::as_str))
            .unwrap_or_default()
            .to_string();
        let thread_id = self
            .provider_thread_id
            .as_deref()
            .filter(|value| !value.is_empty())
            .or_else(|| self.run_id.as_ref().map(RunId::as_str))
            .unwrap_or_default()
            .to_string();
        BTreeMap::from([
            ("session_id".to_string(), session_id.clone()),
            ("session-id".to_string(), session_id),
            ("thread_id".to_string(), thread_id.clone()),
            ("thread-id".to_string(), thread_id.clone()),
            ("x-client-request-id".to_string(), thread_id),
        ])
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

    /// Render runtime context instructions for model-facing requests.
    #[must_use]
    pub fn inject_runtime_context(&self, is_user_prompt: bool) -> Option<String> {
        runtime_context::render_runtime_context(self, is_user_prompt)
    }

    /// Render context instructions for model-facing user prompts.
    #[must_use]
    pub fn context_instructions(&self, is_user_prompt: bool) -> Option<String> {
        self.inject_runtime_context(is_user_prompt)
    }
}

impl Default for AgentContext {
    fn default() -> Self {
        Self::new(AgentId::default())
    }
}
