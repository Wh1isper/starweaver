//! Named filter capability implementations.

mod capability;
mod context_injection;
mod ordering;
mod reasoning;
mod tool_args;

use std::sync::Arc;

use starweaver_model::{ModelAdapter, ModelRequestParameters, ModelSettings};
use starweaver_runtime::{AgentCapability, DynTraceRecorder, StaticCapabilityBundle};

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
    default_filter_capabilities_with_media_uploader(
        compact_model,
        compact_model_settings,
        compact_request_params,
        None,
        None,
    )
}

pub(super) fn default_filter_capabilities_with_media_uploader(
    compact_model: Option<&Arc<dyn ModelAdapter>>,
    compact_model_settings: Option<&ModelSettings>,
    compact_request_params: Option<&ModelRequestParameters>,
    trace_recorder: Option<&DynTraceRecorder>,
    media_uploader: Option<&Arc<dyn super::media::MediaUploader>>,
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
                if let Some(recorder) = trace_recorder.cloned() {
                    capability = capability.with_trace_recorder(recorder);
                }
                Arc::new(capability) as Arc<dyn AgentCapability>
            } else if *name == "media_upload" {
                media_uploader.map_or_else(
                    || Arc::new(NamedFilterCapability::new(name)) as Arc<dyn AgentCapability>,
                    |uploader| {
                        Arc::new(NamedFilterCapability::media_upload(Arc::clone(uploader)))
                            as Arc<dyn AgentCapability>
                    },
                )
            } else {
                Arc::new(NamedFilterCapability::new(name)) as Arc<dyn AgentCapability>
            }
        })
        .collect()
}
