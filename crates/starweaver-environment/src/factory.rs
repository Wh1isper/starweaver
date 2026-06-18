//! Environment provider and resource restore factories.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;

use crate::{
    DynEnvironmentProvider, EnvironmentError, EnvironmentPolicy, EnvironmentResult,
    EnvironmentState, LocalEnvironmentProvider, ResourceRef, VirtualEnvironmentProvider,
};

/// Metadata key that identifies the provider kind used to restore an environment state.
pub const ENVIRONMENT_PROVIDER_KIND_KEY: &str = "provider_kind";

/// Metadata key that identifies the factory kind used to restore a resource reference.
pub const RESOURCE_REF_KIND_KEY: &str = "resource_kind";

/// Restore factory for one environment provider kind.
pub trait EnvironmentProviderFactory: Send + Sync {
    /// Stable provider kind handled by this factory.
    fn kind(&self) -> &'static str;

    /// Restore a provider from an exported state snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the state is not valid for this factory.
    fn restore(&self, state: &EnvironmentState) -> EnvironmentResult<DynEnvironmentProvider>;
}

/// Shared environment provider factory reference.
pub type DynEnvironmentProviderFactory = Arc<dyn EnvironmentProviderFactory>;

/// Restore factory for one provider-scoped or external resource reference kind.
#[async_trait]
pub trait ResourceRestoreFactory: Send + Sync {
    /// Stable resource kind handled by this factory.
    fn kind(&self) -> &'static str;

    /// Restore or rewrite one exported resource reference.
    ///
    /// # Errors
    ///
    /// Returns an error when the host cannot restore the referenced resource.
    async fn restore(&self, resource: &ResourceRef) -> EnvironmentResult<ResourceRef>;
}

/// Shared resource restore factory reference.
pub type DynResourceRestoreFactory = Arc<dyn ResourceRestoreFactory>;

/// Registry of host-owned resource restore factories.
#[derive(Clone, Default)]
pub struct ResourceRestoreFactoryRegistry {
    factories: BTreeMap<String, DynResourceRestoreFactory>,
}

impl ResourceRestoreFactoryRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one factory.
    #[must_use]
    pub fn with_factory(mut self, factory: DynResourceRestoreFactory) -> Self {
        self.factories.insert(factory.kind().to_string(), factory);
        self
    }

    /// Insert one factory.
    pub fn insert(&mut self, factory: DynResourceRestoreFactory) {
        self.factories.insert(factory.kind().to_string(), factory);
    }

    /// Restore all resources with registered factories, preserving others as references.
    ///
    /// This is the portable path: provider-scoped references that do not need a host
    /// factory remain intact, while host-owned external references can be rewritten
    /// or validated by their registered factory.
    ///
    /// # Errors
    ///
    /// Returns factory errors for resources whose kind is registered.
    pub async fn restore_all(
        &self,
        resources: &[ResourceRef],
    ) -> EnvironmentResult<Vec<ResourceRef>> {
        let mut restored = Vec::with_capacity(resources.len());
        for resource in resources {
            restored.push(
                self.restore_optional(resource)
                    .await?
                    .unwrap_or_else(|| resource.clone()),
            );
        }
        Ok(restored)
    }

    /// Restore typed resources and preserve untyped provider-scoped references.
    ///
    /// A resource with `resource_kind` metadata declares host-owned restore
    /// semantics and therefore requires a matching factory. Untyped references
    /// are treated as provider-scoped references and are preserved as-is.
    ///
    /// # Errors
    ///
    /// Returns an error when a typed resource has no registered factory or when
    /// its factory fails.
    pub async fn restore_typed_all(
        &self,
        resources: &[ResourceRef],
    ) -> EnvironmentResult<Vec<ResourceRef>> {
        let mut restored = Vec::with_capacity(resources.len());
        for resource in resources {
            restored.push(match resource_ref_kind(resource) {
                Some(_) => self.restore_required(resource).await?,
                None => resource.clone(),
            });
        }
        Ok(restored)
    }

    /// Restore all resources and require a registered factory for every typed resource.
    ///
    /// # Errors
    ///
    /// Returns an error when a resource has no kind or has no registered factory.
    pub async fn restore_required_all(
        &self,
        resources: &[ResourceRef],
    ) -> EnvironmentResult<Vec<ResourceRef>> {
        let mut restored = Vec::with_capacity(resources.len());
        for resource in resources {
            restored.push(self.restore_required(resource).await?);
        }
        Ok(restored)
    }

    /// Restore one resource when a matching factory is registered.
    ///
    /// # Errors
    ///
    /// Returns factory errors for resources whose kind is registered.
    pub async fn restore_optional(
        &self,
        resource: &ResourceRef,
    ) -> EnvironmentResult<Option<ResourceRef>> {
        let Some(kind) = resource_ref_kind(resource) else {
            return Ok(None);
        };
        let Some(factory) = self.factories.get(kind) else {
            return Ok(None);
        };
        Ok(Some(factory.restore(resource).await?))
    }

    /// Restore one resource and require a matching registered factory.
    ///
    /// # Errors
    ///
    /// Returns an error when the resource has no kind, has no factory, or the
    /// factory cannot restore it.
    pub async fn restore_required(&self, resource: &ResourceRef) -> EnvironmentResult<ResourceRef> {
        let kind = resource_ref_kind(resource)
            .ok_or_else(|| EnvironmentError::InvalidRequest("missing resource kind".to_string()))?;
        let factory = self.factories.get(kind).ok_or_else(|| {
            EnvironmentError::InvalidRequest(format!(
                "no resource restore factory registered for {kind}"
            ))
        })?;
        factory.restore(resource).await
    }
}

/// Registry of environment provider restore factories.
#[derive(Clone, Default)]
pub struct EnvironmentProviderFactoryRegistry {
    factories: BTreeMap<String, DynEnvironmentProviderFactory>,
}

impl EnvironmentProviderFactoryRegistry {
    /// Create an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a registry with portable built-in factories.
    #[must_use]
    pub fn portable_defaults() -> Self {
        Self::new().with_factory(Arc::new(VirtualEnvironmentProviderFactory))
    }

    /// Add one factory.
    #[must_use]
    pub fn with_factory(mut self, factory: DynEnvironmentProviderFactory) -> Self {
        self.factories.insert(factory.kind().to_string(), factory);
        self
    }

    /// Insert one factory.
    pub fn insert(&mut self, factory: DynEnvironmentProviderFactory) {
        self.factories.insert(factory.kind().to_string(), factory);
    }

    /// Restore a provider using the snapshot's provider kind metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the state lacks a provider kind or no factory is registered.
    pub fn restore(&self, state: &EnvironmentState) -> EnvironmentResult<DynEnvironmentProvider> {
        let kind = environment_provider_kind(state).ok_or_else(|| {
            EnvironmentError::InvalidRequest("missing environment provider kind".to_string())
        })?;
        let factory = self.factories.get(kind).ok_or_else(|| {
            EnvironmentError::InvalidRequest(format!(
                "no environment provider factory registered for {kind}"
            ))
        })?;
        factory.restore(state)
    }
}

/// Return the provider kind recorded on an environment state.
#[must_use]
pub fn environment_provider_kind(state: &EnvironmentState) -> Option<&str> {
    state
        .metadata
        .get(ENVIRONMENT_PROVIDER_KIND_KEY)
        .and_then(serde_json::Value::as_str)
}

/// Return the resource kind recorded on a resource reference.
#[must_use]
pub fn resource_ref_kind(resource: &ResourceRef) -> Option<&str> {
    resource
        .metadata
        .get(RESOURCE_REF_KIND_KEY)
        .and_then(serde_json::Value::as_str)
}

/// Restore factory for portable virtual environment state.
#[derive(Clone, Debug, Default)]
pub struct VirtualEnvironmentProviderFactory;

impl EnvironmentProviderFactory for VirtualEnvironmentProviderFactory {
    fn kind(&self) -> &'static str {
        "virtual"
    }

    fn restore(&self, state: &EnvironmentState) -> EnvironmentResult<DynEnvironmentProvider> {
        Ok(Arc::new(VirtualEnvironmentProvider::from_state(
            state.clone(),
        )?))
    }
}

/// Restore factory for trusted local environment state.
#[derive(Clone, Debug)]
pub struct TrustedLocalEnvironmentProviderFactory {
    policy: EnvironmentPolicy,
}

impl TrustedLocalEnvironmentProviderFactory {
    /// Create a local restore factory with explicit host policy.
    #[must_use]
    pub const fn new(policy: EnvironmentPolicy) -> Self {
        Self { policy }
    }
}

impl EnvironmentProviderFactory for TrustedLocalEnvironmentProviderFactory {
    fn kind(&self) -> &'static str {
        "local"
    }

    fn restore(&self, state: &EnvironmentState) -> EnvironmentResult<DynEnvironmentProvider> {
        Ok(Arc::new(LocalEnvironmentProvider::from_trusted_state(
            state,
            self.policy.clone(),
        )?))
    }
}
