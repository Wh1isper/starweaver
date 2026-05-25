//! Runtime service foundations for Starweaver Claw.

use starweaver_core::sdk_name;

/// Returns the runtime component name.
#[must_use]
pub fn component_name() -> String {
    format!("{}::claw", sdk_name())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_component_name() {
        assert_eq!(component_name(), "starweaver-agent-sdk::claw");
    }
}
