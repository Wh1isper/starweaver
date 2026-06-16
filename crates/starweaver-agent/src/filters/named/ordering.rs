use starweaver_runtime::CapabilityOrdering;

/// Ordered default filter names for SDK request preparation.
pub const DEFAULT_FILTER_ORDER: &[&str] = &[
    "reasoning_normalize",
    "media_split",
    "media_compress",
    "media_preflight",
    "media_upload",
    "tool_args",
    "handoff",
    "auto_load_files",
    "capability",
    "bus_message",
    "background_shell",
    "compact",
    "cold_start",
    "environment_context",
    "auto_load_files_after_compact",
    "runtime_context",
    "system_prompt",
];

pub(super) fn filter_capability_id(name: &str) -> String {
    format!("starweaver.filter.{name}")
}

pub(super) fn filter_capability_ordering(name: &str) -> CapabilityOrdering {
    let Some(index) = DEFAULT_FILTER_ORDER
        .iter()
        .position(|candidate| *candidate == name)
    else {
        return CapabilityOrdering::default();
    };
    let mut ordering = CapabilityOrdering::default();
    if let Some(previous) = index
        .checked_sub(1)
        .and_then(|idx| DEFAULT_FILTER_ORDER.get(idx))
    {
        ordering = ordering.after(filter_capability_id(previous));
    }
    ordering
}
