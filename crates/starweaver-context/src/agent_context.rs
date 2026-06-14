use std::{collections::BTreeMap, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{
    AgentId, ConversationId, Metadata, RunId, TraceContext, Usage, UsageAgentTotal, UsageSnapshot,
    UsageSnapshotEntry,
};
use starweaver_model::ModelMessage;

use crate::{
    runtime_context, task::TASK_STATE_DOMAIN, AgentEvent, BusMessage, DependencyStore, EventBus,
    MessageBus, ModelConfig, NoteStore, ResumableState, SecurityConfig, StateStore, Task,
    TaskSnapshot, ToolConfig, TASK_SNAPSHOT_EVENT_KIND,
};

/// Lifecycle-wide agent context.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AgentContext {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Current run identifier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    /// Conversation identifier.
    pub conversation_id: ConversationId,
    /// Canonical message history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_history: Vec<ModelMessage>,
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
    /// Context creation time used for elapsed runtime context.
    #[serde(default = "Utc::now")]
    pub started_at: DateTime<Utc>,
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
        Self {
            agent_id,
            run_id: None,
            conversation_id: ConversationId::new(),
            message_history: Vec::new(),
            usage: Usage::default(),
            usage_snapshot_entries: BTreeMap::new(),
            model_config: ModelConfig::default(),
            tool_config: ToolConfig::default(),
            security: SecurityConfig::default(),
            started_at: Utc::now(),
            state: StateStore::new(),
            events: EventBus::new(),
            notes: NoteStore::new(),
            messages: MessageBus::new(),
            trace_context: TraceContext::default(),
            metadata: Metadata::default(),
            dependencies: DependencyStore::new(),
        }
    }

    /// Restore a context from serialized state.
    #[must_use]
    pub fn from_state(state: ResumableState) -> Self {
        Self {
            agent_id: state.agent_id,
            run_id: state.run_id,
            conversation_id: state.conversation_id.unwrap_or_default(),
            message_history: state.message_history,
            usage: state.usage,
            usage_snapshot_entries: state.usage_snapshot_entries,
            model_config: state.model_config,
            tool_config: state.tool_config,
            security: state.security,
            started_at: state.started_at,
            state: state.state,
            events: EventBus::new(),
            notes: state.notes,
            messages: state.message_bus,
            trace_context: state.trace_snapshot,
            metadata: state.metadata,
            dependencies: DependencyStore::new(),
        }
    }

    /// Export context state for session restoration.
    #[must_use]
    pub fn export_state(&self) -> ResumableState {
        ResumableState {
            agent_id: self.agent_id.clone(),
            run_id: self.run_id.clone(),
            conversation_id: Some(self.conversation_id.clone()),
            message_history: self.message_history.clone(),
            usage: self.usage.clone(),
            usage_snapshot_entries: self.usage_snapshot_entries.clone(),
            model_config: self.model_config.clone(),
            tool_config: self.tool_config.clone(),
            security: self.security.clone(),
            started_at: self.started_at,
            state: self.state.clone(),
            notes: self.notes.clone(),
            message_bus: self.messages.clone(),
            trace_snapshot: self.trace_context.clone(),
            metadata: self.metadata.clone(),
        }
    }

    /// Replace context with serialized state.
    pub fn restore_state(&mut self, state: ResumableState) {
        *self = Self::from_state(state);
    }

    /// Create a child context for subagent execution.
    ///
    /// The child receives long-lived runtime state needed for delegation: the parent
    /// conversation id, accumulated usage, state domains, notes, and typed dependencies.
    /// Per-run queues and histories start empty so delegated runs have an isolated model
    /// history and do not duplicate pending parent steering messages.
    #[must_use]
    pub fn subagent_context(&self, agent_id: impl Into<String>) -> Self {
        let mut metadata = self.metadata.clone();
        metadata.insert(
            "parent_agent_id".to_string(),
            serde_json::json!(self.agent_id.as_str()),
        );
        if let Some(run_id) = &self.run_id {
            metadata.insert(
                "parent_run_id".to_string(),
                serde_json::json!(run_id.as_str()),
            );
        }
        Self {
            agent_id: AgentId::from_string(agent_id),
            run_id: None,
            conversation_id: self.conversation_id.clone(),
            message_history: Vec::new(),
            usage: self.usage.clone(),
            usage_snapshot_entries: self.usage_snapshot_entries.clone(),
            model_config: self.model_config.clone(),
            tool_config: self.tool_config.clone(),
            security: self.security.clone(),
            started_at: Utc::now(),
            state: self.state.clone(),
            events: EventBus::new(),
            notes: self.notes.clone(),
            messages: MessageBus::new(),
            trace_context: self.trace_context.clone(),
            metadata,
            dependencies: self.dependencies.clone(),
        }
    }

    /// Absorb child context state that should survive successful subagent execution.
    pub fn absorb_subagent_context(&mut self, child: &Self) {
        self.usage = child.usage.clone();
        self.usage_snapshot_entries
            .clone_from(&child.usage_snapshot_entries);
        self.notes = child.notes.clone();
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
                .run_id
                .as_ref()
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

    /// Return all tasks from the context task state domain.
    #[must_use]
    pub fn tasks(&self) -> Vec<Task> {
        self.state
            .get(TASK_STATE_DOMAIN)
            .cloned()
            .and_then(|value| serde_json::from_value::<TaskSnapshot>(value).ok())
            .map_or_else(Vec::new, |snapshot| snapshot.tasks)
    }

    /// Replace all tasks in the context task state domain.
    pub fn set_tasks(&mut self, tasks: Vec<Task>) {
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
