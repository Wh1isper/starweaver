use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, MutexGuard},
};

use starweaver_usage::Usage;

use crate::{AgentContext, AgentEvent, Task, TaskManager, ToolSearchInvalidation, ToolSearchState};

/// Stable grant name for handoff and auto-load context mutations.
pub const CONTEXT_HANDOFF_CAPABILITY: &str = "starweaver.context.handoff";
/// Stable grant name for task-manager mutations and snapshots.
pub const CONTEXT_TASKS_CAPABILITY: &str = "starweaver.context.tasks";
/// Stable grant name for usage-ledger mutations.
pub const CONTEXT_USAGE_CAPABILITY: &str = "starweaver.context.usage";
/// Stable grant name for tool-search state and event mutations.
pub const CONTEXT_TOOL_SEARCH_CAPABILITY: &str = "starweaver.context.tool_search";

/// Shared compatibility handle for tools explicitly using the Legacy dependency profile.
#[derive(Clone)]
pub struct AgentContextHandle {
    inner: Arc<Mutex<AgentContext>>,
}

impl AgentContextHandle {
    /// Create a handle from a complete context snapshot.
    #[must_use]
    pub fn new(context: AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(context)),
        }
    }

    /// Return the latest context snapshot held by this handle.
    #[must_use]
    pub fn snapshot(&self) -> AgentContext {
        lock(&self.inner).clone()
    }

    /// Replace the context snapshot held by this handle.
    pub fn replace(&self, context: AgentContext) {
        *lock(&self.inner) = context;
    }

    /// Mutate the context snapshot held by this handle.
    pub fn update(&self, update: impl FnOnce(&mut AgentContext)) {
        update(&mut lock(&self.inner));
    }
}

impl Default for AgentContextHandle {
    fn default() -> Self {
        Self::new(AgentContext::default())
    }
}

impl std::fmt::Debug for AgentContextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentContextHandle")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

#[derive(Clone, Debug)]
struct HandoffMutation {
    handoff_message: Option<String>,
    auto_load_files: Vec<String>,
}

/// Capability-specific mutable handle for context handoff state.
#[derive(Clone)]
pub struct ContextHandoffHandle {
    inner: Arc<Mutex<HandoffMutation>>,
}

impl ContextHandoffHandle {
    /// Create a narrow handoff handle from current context values.
    #[must_use]
    pub fn from_context(context: &AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(HandoffMutation {
                handoff_message: context.handoff_message.clone(),
                auto_load_files: context.tools.auto_load_files.clone(),
            })),
        }
    }

    /// Set the rendered handoff and merge auto-load file paths.
    pub fn set_handoff(&self, rendered: String, auto_load_files: &[String]) {
        let mut mutation = lock(&self.inner);
        mutation.handoff_message = Some(rendered);
        for file in auto_load_files {
            if !mutation.auto_load_files.contains(file) {
                mutation.auto_load_files.push(file.clone());
            }
        }
    }

    /// Apply the isolated handoff state to a context.
    pub fn apply_to(&self, context: &mut AgentContext) {
        let mutation = lock(&self.inner);
        context
            .handoff_message
            .clone_from(&mutation.handoff_message);
        context
            .tools
            .auto_load_files
            .clone_from(&mutation.auto_load_files);
    }
}

/// Capability-specific mutable handle for task-manager state.
#[derive(Clone)]
pub struct TaskContextHandle {
    inner: Arc<Mutex<TaskManager>>,
}

impl TaskContextHandle {
    /// Create a narrow task handle from current context values.
    #[must_use]
    pub fn from_context(context: &AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(context.tools.tasks.clone())),
        }
    }

    /// Mutate only the task manager and return its latest snapshot.
    pub fn update<R>(&self, update: impl FnOnce(&mut TaskManager) -> R) -> (R, Vec<Task>) {
        let mut manager = lock(&self.inner);
        let result = update(&mut manager);
        let snapshot = manager.list_all();
        drop(manager);
        (result, snapshot)
    }

    /// Read the current task snapshot.
    #[must_use]
    pub fn snapshot(&self) -> Vec<Task> {
        lock(&self.inner).list_all()
    }

    /// Apply the isolated task state and publish its durable snapshot to a context.
    pub fn apply_to(&self, context: &mut AgentContext) {
        context.tools.tasks = lock(&self.inner).clone();
        let snapshot = context.tools.tasks.list_all();
        context.set_tasks(snapshot);
        context.publish_task_snapshot_event();
    }
}

/// Capability-specific mutable handle for usage accounting.
#[derive(Clone)]
pub struct UsageContextHandle {
    inner: Arc<Mutex<Usage>>,
}

impl UsageContextHandle {
    /// Create a narrow usage handle from current context values.
    #[must_use]
    pub fn from_context(context: &AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(context.usage.clone())),
        }
    }

    /// Add usage to the isolated usage cell.
    pub fn add_usage(&self, usage: &Usage) {
        lock(&self.inner).add_assign(usage);
    }

    fn apply_to(&self, context: &mut AgentContext) {
        context.usage = lock(&self.inner).clone();
    }
}

#[derive(Clone, Debug)]
struct ToolSearchMutation {
    state: ToolSearchState,
    events: Vec<AgentEvent>,
}

/// Capability-specific mutable handle for tool-search state and sideband events.
#[derive(Clone)]
pub struct ToolSearchContextHandle {
    inner: Arc<Mutex<ToolSearchMutation>>,
}

impl ToolSearchContextHandle {
    /// Create a narrow tool-search handle from current context values.
    #[must_use]
    pub fn from_context(context: &AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(ToolSearchMutation {
                state: context.tool_search_state(),
                events: Vec::new(),
            })),
        }
    }

    /// Return the current loaded tool-search state.
    #[must_use]
    pub fn state(&self) -> ToolSearchState {
        lock(&self.inner).state.clone()
    }

    /// Record loaded tools and namespaces.
    pub fn record_loaded(
        &self,
        tools: impl IntoIterator<Item = impl Into<String>>,
        namespaces: impl IntoIterator<Item = impl Into<String>>,
    ) {
        let mut mutation = lock(&self.inner);
        for tool in tools {
            push_unique(&mut mutation.state.loaded_tools, tool.into());
        }
        for namespace in namespaces {
            push_unique(&mut mutation.state.loaded_namespaces, namespace.into());
        }
    }

    /// Clear all loaded search state.
    #[must_use]
    pub fn clear_loaded(&self) -> ToolSearchInvalidation {
        let mut mutation = lock(&self.inner);
        ToolSearchInvalidation {
            removed_tools: std::mem::take(&mut mutation.state.loaded_tools),
            removed_namespaces: std::mem::take(&mut mutation.state.loaded_namespaces),
        }
    }

    /// Publish a tool-search sideband event for absorption after tool execution.
    pub fn publish_event(&self, event: AgentEvent) {
        lock(&self.inner).events.push(event);
    }

    /// Apply isolated tool-search state and events to a context.
    pub fn apply_to(&self, context: &mut AgentContext) {
        let mutation = lock(&self.inner);
        context
            .tools
            .loaded_tool_names
            .clone_from(&mutation.state.loaded_tools);
        context
            .tools
            .loaded_tool_namespaces
            .clone_from(&mutation.state.loaded_namespaces);
        for event in &mutation.events {
            context.publish_event(event.clone());
        }
    }
}

/// Runtime-owned collection of isolated context mutation cells for one tool call.
#[derive(Clone)]
pub struct ContextMutationHandles {
    handoff: ContextHandoffHandle,
    tasks: TaskContextHandle,
    usage: UsageContextHandle,
    tool_search: ToolSearchContextHandle,
}

impl ContextMutationHandles {
    /// Capture only the domains that narrow handles can mutate.
    #[must_use]
    pub fn from_context(context: &AgentContext) -> Self {
        Self {
            handoff: ContextHandoffHandle::from_context(context),
            tasks: TaskContextHandle::from_context(context),
            usage: UsageContextHandle::from_context(context),
            tool_search: ToolSearchContextHandle::from_context(context),
        }
    }

    /// Insert runtime-recognized capability handles into a dependency store.
    pub fn insert_grants(
        &self,
        dependencies: &mut crate::DependencyStore,
        requested: &BTreeSet<String>,
    ) {
        for capability in requested {
            match capability.as_str() {
                CONTEXT_HANDOFF_CAPABILITY => dependencies.insert(self.handoff.clone()),
                CONTEXT_TASKS_CAPABILITY => dependencies.insert(self.tasks.clone()),
                CONTEXT_USAGE_CAPABILITY => dependencies.insert(self.usage.clone()),
                CONTEXT_TOOL_SEARCH_CAPABILITY => dependencies.insert(self.tool_search.clone()),
                _ => {}
            }
        }
    }

    /// Apply only explicitly authorized mutation domains back to the runtime context.
    pub fn apply_to(&self, context: &mut AgentContext, authorized: &BTreeSet<String>) {
        for capability in authorized {
            match capability.as_str() {
                CONTEXT_HANDOFF_CAPABILITY => self.handoff.apply_to(context),
                CONTEXT_TASKS_CAPABILITY => self.tasks.apply_to(context),
                CONTEXT_USAGE_CAPABILITY => self.usage.apply_to(context),
                CONTEXT_TOOL_SEARCH_CAPABILITY => self.tool_search.apply_to(context),
                _ => {}
            }
        }
    }
}

impl std::fmt::Debug for ContextHandoffHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ContextHandoffHandle")
    }
}

impl std::fmt::Debug for TaskContextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TaskContextHandle")
    }
}

impl std::fmt::Debug for UsageContextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("UsageContextHandle")
    }
}

impl std::fmt::Debug for ToolSearchContextHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ToolSearchContextHandle")
    }
}

impl std::fmt::Debug for ContextMutationHandles {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ContextMutationHandles")
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}
