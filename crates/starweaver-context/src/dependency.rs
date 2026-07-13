//! Type-indexed dependency container for runtime and tool contexts.

use std::{
    any::{Any, TypeId},
    collections::{BTreeMap, BTreeSet},
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
    ///
    /// The inserted name becomes the preferred typed lookup for `T`; older
    /// names remain available through named lookup.
    pub fn insert_named<T>(&mut self, name: impl Into<String>, value: T)
    where
        T: Send + Sync + 'static,
    {
        self.insert_named_arc(name, Arc::new(value));
    }

    /// Insert an already shared dependency with a caller-provided stable name.
    ///
    /// Replacing a preferred name with another type reindexes any remaining
    /// alias for the previous type instead of leaving a stale typed mapping.
    pub fn insert_named_arc<T>(&mut self, name: impl Into<String>, value: Arc<T>)
    where
        T: Send + Sync + 'static,
    {
        let name = name.into();
        if let Some(previous) = self.values.get(&name) {
            let previous_type = previous.as_ref().type_id();
            if self.type_keys.get(&previous_type) == Some(&name) {
                self.type_keys.remove(&previous_type);
                if let Some(fallback_name) = self
                    .values
                    .iter()
                    .filter(|(candidate_name, candidate)| {
                        *candidate_name != &name && candidate.as_ref().type_id() == previous_type
                    })
                    .map(|(candidate_name, _)| candidate_name.clone())
                    .next_back()
                {
                    self.type_keys.insert(previous_type, fallback_name);
                }
            }
        }
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

    /// Clone a dependency subset selected by stable names.
    ///
    /// Typed lookup remains available when the selected name is not the source
    /// store's preferred alias for that type. If several selected names contain
    /// the same type, the source store's preferred alias wins when selected;
    /// otherwise stable name order determines the fallback alias.
    #[must_use]
    pub fn subset(&self, names: &BTreeSet<String>) -> Self {
        let values: BTreeMap<_, _> = self
            .values
            .iter()
            .filter(|(name, _)| names.contains(*name))
            .map(|(name, value)| (name.clone(), Arc::clone(value)))
            .collect();
        let mut type_keys = BTreeMap::new();
        for (name, value) in &values {
            type_keys.insert(value.as_ref().type_id(), name.clone());
        }
        for (type_id, name) in &self.type_keys {
            if values.contains_key(name) {
                type_keys.insert(*type_id, name.clone());
            }
        }
        Self { values, type_keys }
    }

    /// Merge dependencies from another store, replacing matching stable names and typed lookups.
    pub fn extend(&mut self, other: Self) {
        for (name, value) in other.values {
            if let Some(previous) = self.values.insert(name.clone(), value.clone()) {
                let previous_type = previous.as_ref().type_id();
                if self.type_keys.get(&previous_type) == Some(&name) {
                    self.type_keys.remove(&previous_type);
                }
            }
            self.type_keys.insert(value.as_ref().type_id(), name);
        }
        for (type_id, name) in other.type_keys {
            if self.values.contains_key(&name) {
                self.type_keys.insert(type_id, name);
            }
        }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::DependencyStore;

    #[derive(Debug, Eq, PartialEq)]
    struct First(u32);

    #[derive(Debug, Eq, PartialEq)]
    struct Second(u32);

    #[test]
    fn subset_preserves_selected_typed_and_named_lookups() {
        let mut store = DependencyStore::new();
        store.insert(First(1));
        store.insert_named("second", Second(2));
        let names = BTreeSet::from([
            std::any::type_name::<First>().to_string(),
            "second".to_string(),
        ]);

        let subset = store.subset(&names);

        assert_eq!(subset.get::<First>().as_deref(), Some(&First(1)));
        assert_eq!(subset.get::<Second>().as_deref(), Some(&Second(2)));
        assert_eq!(
            subset.get_named::<Second>("second").as_deref(),
            Some(&Second(2))
        );
    }

    #[test]
    fn subset_removes_omitted_type_keys_and_values() {
        let mut store = DependencyStore::new();
        store.insert(First(1));
        store.insert(Second(2));
        let names = BTreeSet::from([std::any::type_name::<First>().to_string()]);

        let subset = store.subset(&names);

        assert!(subset.get::<First>().is_some());
        assert!(subset.get::<Second>().is_none());
        assert!(
            subset
                .get_named::<Second>(std::any::type_name::<Second>())
                .is_none()
        );
    }

    #[test]
    fn replacing_a_name_with_another_type_removes_the_stale_typed_lookup() {
        let mut store = DependencyStore::new();
        store.insert_named("shared", First(1));
        store.insert_named("shared", Second(2));

        assert!(store.get::<First>().is_none());
        assert_eq!(store.get::<Second>().as_deref(), Some(&Second(2)));
        assert_eq!(
            store.get_named::<Second>("shared").as_deref(),
            Some(&Second(2))
        );
    }

    #[test]
    fn replacing_the_preferred_alias_reindexes_an_existing_alias() {
        let mut store = DependencyStore::new();
        store.insert_named("fallback", First(1));
        store.insert_named("preferred", First(2));
        store.insert_named("preferred", Second(3));

        assert_eq!(store.get::<First>().as_deref(), Some(&First(1)));
        assert_eq!(store.get::<Second>().as_deref(), Some(&Second(3)));
    }

    #[test]
    fn subset_reindexes_a_selected_non_preferred_alias_for_typed_lookup() {
        let mut store = DependencyStore::new();
        store.insert_named("first-alias", First(1));
        store.insert_named("preferred-alias", First(2));
        assert_eq!(store.get::<First>().as_deref(), Some(&First(2)));

        let subset = store.subset(&BTreeSet::from(["first-alias".to_string()]));

        assert_eq!(subset.get::<First>().as_deref(), Some(&First(1)));
        assert_eq!(
            subset.get_named::<First>("first-alias").as_deref(),
            Some(&First(1))
        );
    }
}
