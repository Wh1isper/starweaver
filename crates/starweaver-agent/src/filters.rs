//! Named SDK history filter presets for ya-agent-sdk parity.

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
