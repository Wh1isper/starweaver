//! Named filter capability implementations.

mod capability;
mod context_injection;
mod ordering;
mod reasoning;
mod tool_args;

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_runtime::{AgentCapability, StaticCapabilityBundle};

use super::compact::CacheFriendlyCompactCapability;

pub use capability::NamedFilterCapability;
pub use ordering::DEFAULT_FILTER_ORDER;

/// Build the default named filter bundle.
#[must_use]
pub fn default_filter_bundle() -> StaticCapabilityBundle {
    let mut bundle = StaticCapabilityBundle::new("starweaver-default-filters");
    for capability in default_filter_capabilities(None) {
        bundle = bundle.with_hook(capability);
    }
    bundle
}

/// Build named filter capabilities in default order.
#[must_use]
pub fn default_filter_capabilities(
    compact_model: Option<&Arc<dyn ModelAdapter>>,
) -> Vec<Arc<dyn AgentCapability>> {
    default_filter_capabilities_with_config(compact_model, None, None)
}

/// Build named filter capabilities with inherited compactor configuration.
#[must_use]
pub fn default_filter_capabilities_with_config(
    compact_model: Option<&Arc<dyn ModelAdapter>>,
    compact_model_settings: Option<&ModelSettings>,
    compact_request_params: Option<&ModelRequestParameters>,
) -> Vec<Arc<dyn AgentCapability>> {
    DEFAULT_FILTER_ORDER
        .iter()
        .map(|name| {
            if *name == "compact" {
                let mut capability = CacheFriendlyCompactCapability::new(compact_model.cloned());
                if let Some(settings) = compact_model_settings.cloned() {
                    capability = capability.with_model_settings(settings);
                }
                if let Some(params) = compact_request_params.cloned() {
                    capability = capability.with_request_params(params);
                }
                Arc::new(capability) as Arc<dyn AgentCapability>
            } else {
                Arc::new(NamedFilterCapability::new(name)) as Arc<dyn AgentCapability>
            }
        })
        .collect()
}
