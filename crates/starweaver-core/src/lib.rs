//! Core abstractions for the Starweaver agent SDK.

/// Workspace-wide SDK identity.
pub const SDK_NAME: &str = "starweaver-agent-sdk";

/// Returns the SDK name used across commands and diagnostics.
#[must_use]
pub const fn sdk_name() -> &'static str {
    SDK_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_sdk_name() {
        assert_eq!(sdk_name(), "starweaver-agent-sdk");
    }
}
