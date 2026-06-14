use std::sync::{Arc, Mutex};

use crate::AgentContext;

/// Shared context snapshot handle for tools that need to report context mutations.
#[derive(Clone)]
pub struct AgentContextHandle {
    inner: Arc<Mutex<AgentContext>>,
}

impl AgentContextHandle {
    /// Create a handle from a context snapshot.
    #[must_use]
    pub fn new(context: AgentContext) -> Self {
        Self {
            inner: Arc::new(Mutex::new(context)),
        }
    }

    /// Return the latest context snapshot held by this handle.
    #[must_use]
    pub fn snapshot(&self) -> AgentContext {
        match self.inner.lock() {
            Ok(context) => context.clone(),
            Err(error) => error.into_inner().clone(),
        }
    }

    /// Replace the context snapshot held by this handle.
    pub fn replace(&self, context: AgentContext) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = context,
            Err(error) => {
                let mut guard = error.into_inner();
                *guard = context;
            }
        }
    }

    /// Mutate the context snapshot held by this handle.
    pub fn update(&self, update: impl FnOnce(&mut AgentContext)) {
        match self.inner.lock() {
            Ok(mut guard) => update(&mut guard),
            Err(error) => {
                let mut guard = error.into_inner();
                update(&mut guard);
            }
        }
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
