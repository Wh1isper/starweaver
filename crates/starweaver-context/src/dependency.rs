//! Type-indexed dependency container for runtime and tool contexts.

use std::{
    any::{Any, TypeId},
    collections::BTreeMap,
    sync::Arc,
};

/// Type-indexed dependency container for runtime and tool contexts.
#[derive(Clone, Default)]
pub struct DependencyStore {
    values: BTreeMap<String, Arc<dyn Any + Send + Sync>>,
    type_keys: BTreeMap<TypeId, String>,
}

impl DependencyStore {
    /// Create an empty dependency store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a dependency using its Rust type as the lookup key.
    pub fn insert<T>(&mut self, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named(std::any::type_name::<T>(), value);
    }

    /// Insert an already shared dependency using its Rust type as the lookup key.
    pub fn insert_arc<T>(&mut self, value: Arc<T>)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named_arc(std::any::type_name::<T>(), value);
    }

    /// Insert a dependency with a caller-provided stable name.
    pub fn insert_named<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named_arc(name, Arc::new(value));
    }

    /// Insert an already shared dependency with a caller-provided stable name.
    pub fn insert_named_arc<T>(&mut self, name: impl Into<String>, value: Arc<T>)
    where
        T: Send + Sync + 'static,
    {
        let name = name.into();
        self.type_keys.insert(TypeId::of::<T>(), name.clone());
        self.values.insert(name, value);
    }

    /// Get a dependency by Rust type.
    #[must_use]
    pub fn get<T>(&self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.type_keys
            .get(&TypeId::of::<T>())
            .and_then(|name| self.get_named(name))
    }

    /// Get a dependency by stable name.
    #[must_use]
    pub fn get_named<T>(&self, name: &str) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.values
            .get(name)
            .cloned()
            .and_then(|value| value.downcast::<T>().ok())
    }

    /// Return all named dependency keys.
    #[must_use]
    pub fn keys(&self) -> Vec<String> {
        self.values.keys().cloned().collect()
    }

    /// Return whether the store has no dependencies.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl std::fmt::Debug for DependencyStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DependencyStore")
            .field("keys", &self.keys())
            .finish()
    }
}
