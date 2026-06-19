//! Named SDK history filter presets for Starweaver.

mod compact;
mod media;
mod message;
mod named;

pub use compact::CacheFriendlyCompactCapability;
pub use media::{MediaUploadRequest, MediaUploader};
pub use named::{
    default_filter_bundle, default_filter_capabilities, default_filter_capabilities_with_config,
    NamedFilterCapability, DEFAULT_FILTER_ORDER,
};

pub(crate) fn default_filter_capabilities_with_media_uploader(
    compact_model: Option<&std::sync::Arc<dyn starweaver_model::ModelAdapter>>,
    compact_model_settings: Option<&starweaver_model::ModelSettings>,
    compact_request_params: Option<&starweaver_model::ModelRequestParameters>,
    trace_recorder: Option<&starweaver_runtime::DynTraceRecorder>,
    media_uploader: Option<&std::sync::Arc<dyn media::MediaUploader>>,
) -> Vec<std::sync::Arc<dyn starweaver_runtime::AgentCapability>> {
    named::default_filter_capabilities_with_media_uploader(
        compact_model,
        compact_model_settings,
        compact_request_params,
        trace_recorder,
        media_uploader,
    )
}

fn filter_capability_id(name: &str) -> String {
    format!("starweaver.filter.{name}")
}

fn filter_capability_ordering(name: &str) -> starweaver_runtime::CapabilityOrdering {
    let Some(index) = DEFAULT_FILTER_ORDER
        .iter()
        .position(|candidate| *candidate == name)
    else {
        return starweaver_runtime::CapabilityOrdering::default();
    };
    let mut ordering = starweaver_runtime::CapabilityOrdering::default();
    if let Some(previous) = index
        .checked_sub(1)
        .and_then(|idx| DEFAULT_FILTER_ORDER.get(idx))
    {
        ordering = ordering.after(filter_capability_id(previous));
    }
    ordering
}
