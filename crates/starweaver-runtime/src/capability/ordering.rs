//! Capability ordering resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use thiserror::Error;

use super::{AgentCapability, CapabilityId};

/// Capability ordering diagnostics.
#[derive(Debug, Error)]
pub enum CapabilityOrderError {
    /// Capability ids must be unique inside one run graph.
    #[error("capability id '{0}' is duplicated")]
    DuplicateId(String),
    /// Ordering constraint referenced a missing capability id.
    #[error("capability '{capability}' references missing dependency '{dependency}'")]
    MissingDependency {
        /// Capability that declared the dependency.
        capability: String,
        /// Missing dependency id.
        dependency: String,
    },
    /// Ordering constraints contain a cycle.
    #[error("capability ordering cycle detected among {0}")]
    Cycle(String),
}

/// Resolve capability order from stable specs.
///
/// # Errors
///
/// Returns duplicate-id, missing-dependency, or cycle diagnostics.
pub fn resolve_capability_order(
    capabilities: &[Arc<dyn AgentCapability>],
) -> Result<Vec<Arc<dyn AgentCapability>>, CapabilityOrderError> {
    let mut ids = Vec::with_capacity(capabilities.len());
    let mut by_id = BTreeMap::new();
    for (index, capability) in capabilities.iter().enumerate() {
        let id = capability.spec().id;
        if by_id.insert(id.clone(), index).is_some() {
            return Err(CapabilityOrderError::DuplicateId(id.as_str().to_string()));
        }
        ids.push(id);
    }

    let mut outgoing = BTreeMap::<CapabilityId, BTreeSet<CapabilityId>>::new();
    let mut incoming = BTreeMap::<CapabilityId, usize>::new();
    for id in &ids {
        outgoing.entry(id.clone()).or_default();
        incoming.entry(id.clone()).or_default();
    }

    for (index, capability) in capabilities.iter().enumerate() {
        let spec = capability.spec();
        let current = ids[index].clone();
        for dependency in spec.ordering.after {
            if !by_id.contains_key(&dependency) {
                return Err(CapabilityOrderError::MissingDependency {
                    capability: current.as_str().to_string(),
                    dependency: dependency.as_str().to_string(),
                });
            }
            if outgoing
                .entry(dependency.clone())
                .or_default()
                .insert(current.clone())
            {
                *incoming.entry(current.clone()).or_default() += 1;
            }
        }
        for target in spec.ordering.before {
            if !by_id.contains_key(&target) {
                return Err(CapabilityOrderError::MissingDependency {
                    capability: current.as_str().to_string(),
                    dependency: target.as_str().to_string(),
                });
            }
            if outgoing
                .entry(current.clone())
                .or_default()
                .insert(target.clone())
            {
                *incoming.entry(target).or_default() += 1;
            }
        }
    }

    let mut emitted = BTreeSet::<CapabilityId>::new();
    let mut ordered = Vec::with_capacity(capabilities.len());
    while ordered.len() < capabilities.len() {
        let Some(next) = ids
            .iter()
            .find(|id| !emitted.contains(*id) && incoming.get(*id).copied().unwrap_or(0) == 0)
            .cloned()
        else {
            let cycle = ids
                .iter()
                .filter(|id| !emitted.contains(*id))
                .map(|id| id.as_str().to_string())
                .collect::<Vec<_>>()
                .join(",");
            return Err(CapabilityOrderError::Cycle(cycle));
        };
        emitted.insert(next.clone());
        let index = by_id[&next];
        ordered.push(capabilities[index].clone());
        if let Some(targets) = outgoing.get(&next) {
            for target in targets {
                if let Some(count) = incoming.get_mut(target) {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }
    Ok(ordered)
}
