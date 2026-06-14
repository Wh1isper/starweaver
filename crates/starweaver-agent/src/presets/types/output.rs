use serde::{Deserialize, Serialize};
use starweaver_runtime::{OutputPolicy, OutputSchema};

/// Serializable output profile for an agent spec.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputSpec {
    /// Optional structured output schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<OutputSchema>,
    /// Optional output validation retry budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retries: Option<usize>,
}

impl OutputSpec {
    pub(in crate::presets) fn to_policy(&self) -> Option<OutputPolicy> {
        if self.schema.is_none() && self.retries.is_none() {
            return None;
        }
        let mut policy = OutputPolicy::new();
        if let Some(schema) = self.schema.clone() {
            policy = policy.with_schema(schema);
        }
        if let Some(retries) = self.retries {
            policy = policy.with_retries(retries);
        }
        Some(policy)
    }
}
