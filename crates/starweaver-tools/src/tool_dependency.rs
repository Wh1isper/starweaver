//! Tool-declared dependency projection requirements.

use std::collections::BTreeSet;

use serde_json::{Map, Value};
use starweaver_core::Metadata;

/// Reserved metadata key for tool dependency projection requirements.
pub const TOOL_METADATA_DEPENDENCIES_KEY: &str = "starweaver_tool_dependencies";

/// Compatibility profile used when assembling one tool's dependencies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolDependencyProfile {
    /// Preserve the released broad dependency projection.
    #[default]
    Legacy,
    /// Omit runtime-generated broad mutable context and filter generated projections.
    Filtered,
    /// Expose only explicitly requested generated projections and capability grants.
    Strict,
}

/// Tool-declared requirements used to build generated dependency projections.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ToolDependencyRequirements {
    /// Compatibility profile for dependency assembly.
    pub profile: ToolDependencyProfile,
    /// Stable dependency names visible through generated host capabilities.
    pub host_capabilities: BTreeSet<String>,
    /// Runtime-authorized mutable context capability grants.
    pub context_capabilities: BTreeSet<String>,
    /// Whether configured shell environment values are required.
    pub shell_environment: bool,
}

impl ToolDependencyRequirements {
    /// Build explicit Legacy compatibility requirements with narrow context capabilities.
    ///
    /// This preserves ambient dependencies for compatibility wrappers while making any
    /// runtime-owned mutable context domains explicit rather than relying on the broad handle.
    #[must_use]
    pub fn legacy_with_context_capabilities(
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            profile: ToolDependencyProfile::Legacy,
            host_capabilities: BTreeSet::new(),
            context_capabilities: capabilities.into_iter().map(Into::into).collect(),
            shell_environment: false,
        }
    }

    /// Build filtered dependency requirements.
    #[must_use]
    pub fn filtered(
        host_capabilities: impl IntoIterator<Item = impl Into<String>>,
        shell_environment: bool,
    ) -> Self {
        Self {
            profile: ToolDependencyProfile::Filtered,
            host_capabilities: host_capabilities.into_iter().map(Into::into).collect(),
            context_capabilities: BTreeSet::new(),
            shell_environment,
        }
    }

    /// Build grant-intersected filtered dependency requirements.
    ///
    /// This is the strict dependency profile with no mutable context capabilities:
    /// named host capabilities are intersected with the current per-tool grant and
    /// ambient application dependencies are excluded.
    #[must_use]
    pub fn granted_filtered(
        host_capabilities: impl IntoIterator<Item = impl Into<String>>,
        shell_environment: bool,
    ) -> Self {
        Self::strict(host_capabilities, Vec::<String>::new(), shell_environment)
    }

    /// Add mutable context capability requests to filtered requirements.
    #[must_use]
    pub fn with_context_capabilities(
        mut self,
        capabilities: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.context_capabilities = capabilities.into_iter().map(Into::into).collect();
        self
    }

    /// Build strict least-authority dependency requirements.
    #[must_use]
    pub fn strict(
        host_capabilities: impl IntoIterator<Item = impl Into<String>>,
        context_capabilities: impl IntoIterator<Item = impl Into<String>>,
        shell_environment: bool,
    ) -> Self {
        Self {
            profile: ToolDependencyProfile::Strict,
            host_capabilities: host_capabilities.into_iter().map(Into::into).collect(),
            context_capabilities: context_capabilities.into_iter().map(Into::into).collect(),
            shell_environment,
        }
    }

    /// Encode these requirements as the reserved metadata value.
    #[must_use]
    pub fn to_metadata_value(&self) -> Value {
        let profile = match self.profile {
            ToolDependencyProfile::Legacy => "legacy",
            ToolDependencyProfile::Filtered => "filtered",
            ToolDependencyProfile::Strict => "strict",
        };
        Value::Object(Map::from_iter([
            ("profile".to_string(), Value::String(profile.to_string())),
            (
                "host_capabilities".to_string(),
                Value::Array(
                    self.host_capabilities
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            (
                "context_capabilities".to_string(),
                Value::Array(
                    self.context_capabilities
                        .iter()
                        .cloned()
                        .map(Value::String)
                        .collect(),
                ),
            ),
            (
                "shell_environment".to_string(),
                Value::Bool(self.shell_environment),
            ),
        ]))
    }
}

/// Error returned when reserved dependency metadata is malformed.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("invalid {TOOL_METADATA_DEPENDENCIES_KEY} metadata")]
pub struct ToolDependencyRequirementsError;

/// Parse and validate tool dependency requirements.
///
/// Missing metadata is a valid Legacy declaration. Unlike the compatibility parser, malformed
/// metadata remains distinguishable and therefore cannot be mistaken for `CodeAct`-safe Strict
/// metadata.
///
/// # Errors
///
/// Returns an error when the reserved metadata value is malformed.
pub fn try_tool_dependency_requirements(
    metadata: &Metadata,
) -> Result<ToolDependencyRequirements, ToolDependencyRequirementsError> {
    let Some(raw) = metadata.get(TOOL_METADATA_DEPENDENCIES_KEY) else {
        return Ok(ToolDependencyRequirements::default());
    };
    let Value::Object(value) = raw else {
        return Err(ToolDependencyRequirementsError);
    };
    let profile = match value.get("profile").and_then(Value::as_str) {
        Some("legacy") => ToolDependencyProfile::Legacy,
        Some("filtered") => ToolDependencyProfile::Filtered,
        Some("strict") => ToolDependencyProfile::Strict,
        _ => return Err(ToolDependencyRequirementsError),
    };
    let Some(host_capabilities) = value.get("host_capabilities").and_then(Value::as_array) else {
        return Err(ToolDependencyRequirementsError);
    };
    let Some(shell_environment) = value.get("shell_environment").and_then(Value::as_bool) else {
        return Err(ToolDependencyRequirementsError);
    };
    let mut names = BTreeSet::new();
    for name in host_capabilities {
        let Some(name) = name.as_str() else {
            return Err(ToolDependencyRequirementsError);
        };
        names.insert(name.to_string());
    }
    let mut context_capabilities = BTreeSet::new();
    if let Some(values) = value.get("context_capabilities") {
        let Some(values) = values.as_array() else {
            return Err(ToolDependencyRequirementsError);
        };
        for name in values {
            let Some(name) = name.as_str() else {
                return Err(ToolDependencyRequirementsError);
            };
            context_capabilities.insert(name.to_string());
        }
    }
    Ok(ToolDependencyRequirements {
        profile,
        host_capabilities: names,
        context_capabilities,
        shell_environment,
    })
}

/// Parse tool dependency requirements with compatibility fail-closed behavior.
///
/// Absence preserves the Legacy compatibility profile. A present but malformed reserved value
/// fails closed to a Strict profile with no requested authority.
#[must_use]
pub fn tool_dependency_requirements(metadata: &Metadata) -> ToolDependencyRequirements {
    try_tool_dependency_requirements(metadata).unwrap_or_else(|_| denied_requirements())
}

fn denied_requirements() -> ToolDependencyRequirements {
    ToolDependencyRequirements::strict(Vec::<String>::new(), Vec::<String>::new(), false)
}

#[cfg(test)]
mod tests {
    use starweaver_core::Metadata;

    use super::*;

    #[test]
    fn missing_requirements_preserve_legacy_and_malformed_requirements_fail_closed() {
        assert_eq!(
            tool_dependency_requirements(&Metadata::new()),
            ToolDependencyRequirements::default()
        );
        let denied = denied_requirements();
        for malformed in [
            serde_json::json!("filtered"),
            serde_json::json!({"profile": "unknown", "host_capabilities": [], "shell_environment": false}),
            serde_json::json!({"profile": "filtered", "host_capabilities": [1], "shell_environment": false}),
            serde_json::json!({"profile": "filtered", "host_capabilities": []}),
        ] {
            let mut metadata = Metadata::new();
            metadata.insert(TOOL_METADATA_DEPENDENCIES_KEY.to_string(), malformed);
            assert_eq!(tool_dependency_requirements(&metadata), denied);
        }
    }

    #[test]
    fn filtered_requirements_round_trip_through_metadata_value() {
        let expected = ToolDependencyRequirements::filtered(
            ["starweaver.environment", "starweaver.shell_review"],
            true,
        );
        let mut metadata = Metadata::new();
        metadata.insert(
            TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
            expected.to_metadata_value(),
        );

        assert_eq!(tool_dependency_requirements(&metadata), expected);
    }

    #[test]
    fn filtered_requirements_ignore_unknown_extension_fields() {
        let mut metadata = Metadata::new();
        metadata.insert(
            TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
            serde_json::json!({
                "profile": "filtered",
                "host_capabilities": ["weather"],
                "shell_environment": false,
                "future_grant": {"revision": 2},
            }),
        );

        assert_eq!(
            tool_dependency_requirements(&metadata),
            ToolDependencyRequirements::filtered(["weather"], false)
        );
    }

    #[test]
    fn explicit_legacy_requirements_round_trip_with_context_grants() {
        let expected = ToolDependencyRequirements::legacy_with_context_capabilities([
            "starweaver.context.tool_search",
        ]);
        let mut metadata = Metadata::new();
        metadata.insert(
            TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
            expected.to_metadata_value(),
        );

        assert_eq!(tool_dependency_requirements(&metadata), expected);
    }

    #[test]
    fn strict_requirements_round_trip_with_context_grants() {
        let expected = ToolDependencyRequirements::strict(
            ["starweaver.environment"],
            ["starweaver.context.tasks"],
            false,
        );
        let mut metadata = Metadata::new();
        metadata.insert(
            TOOL_METADATA_DEPENDENCIES_KEY.to_string(),
            expected.to_metadata_value(),
        );

        assert_eq!(tool_dependency_requirements(&metadata), expected);
    }
}
