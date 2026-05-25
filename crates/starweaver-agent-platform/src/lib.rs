//! Agent platform foundations for Starweaver.

use starweaver_core::sdk_name;

/// Returns the platform component name.
#[must_use]
pub fn component_name() -> String {
    format!("{}::agent-platform", sdk_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(component_name(), "starweaver-agent-sdk::agent-platform");
    }
}
