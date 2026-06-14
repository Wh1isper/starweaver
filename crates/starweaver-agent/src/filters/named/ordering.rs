use starweaver_runtime::CapabilityOrdering;

/// Ordered default filter names for ya-agent-sdk behavioral parity.
pub const DEFAULT_FILTER_ORDER: &[&str] = &[
    "cold_start",
    "capability",
    "media_preflight",
    "media_compress",
    "media_upload",
    "compact",
    "handoff",
    "auto_load_files",
    "background_shell",
    "bus_message",
    "environment_instructions",
    "runtime_instructions",
    "system_prompt",
    "tool_args",
    "reasoning_normalize",
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
