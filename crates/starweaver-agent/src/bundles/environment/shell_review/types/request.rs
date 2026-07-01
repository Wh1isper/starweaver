use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::policy::{ShellReviewDecision, ShellReviewRiskLevel, risk_level_name};

/// Previous review decision rendered into the reviewer prompt.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewPreviousDecision {
    /// Whether the previous command execution was approved.
    pub approved: bool,
    /// Previous risk level.
    pub risk_level: ShellReviewRiskLevel,
    /// Previous reason.
    #[serde(default)]
    pub reason: String,
    /// Previous command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Previous working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// Execution context submitted to shell review.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewContextSnapshot {
    /// Shell timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// Runtime tool call id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Whether this tool call was already approved by a host.
    #[serde(default)]
    pub tool_call_approved: bool,
    /// Default shell working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_cwd: Option<String>,
    /// Provider-visible allowed paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_paths: Vec<String>,
    /// Shell platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_platform: Option<String>,
    /// Shell executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_executable: Option<String>,
}

impl ShellReviewContextSnapshot {
    /// Build approval-safe metadata.
    #[must_use]
    pub fn to_metadata(&self) -> Value {
        serde_json::json!({
            "timeout_seconds": self.timeout_seconds,
            "tool_call_id": self.tool_call_id,
            "tool_call_approved": self.tool_call_approved,
            "default_cwd": self.default_cwd,
            "allowed_paths": self.allowed_paths,
            "shell_platform": self.shell_platform,
            "shell_executable": self.shell_executable,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellReviewFingerprint {
    command: String,
    cwd: Option<String>,
    background: bool,
    environment_keys: Vec<String>,
}

/// Shell command context submitted to the reviewer.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ShellReviewRequest {
    /// Command string.
    pub command: String,
    /// Optional working directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Whether command runs in background.
    #[serde(default)]
    pub background: bool,
    /// Environment variable keys visible to the command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_keys: Vec<String>,
    /// Execution context snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_snapshot: Option<ShellReviewContextSnapshot>,
    /// Previous shell reviews for consistency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_reviews: Vec<ShellReviewPreviousDecision>,
}

impl ShellReviewRequest {
    pub(crate) fn command_fingerprint(&self) -> ShellReviewFingerprint {
        let mut environment_keys = self.environment_keys.clone();
        environment_keys.sort();
        ShellReviewFingerprint {
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            background: self.background,
            environment_keys,
        }
    }

    /// Render the model prompt for this review request.
    #[must_use]
    pub fn to_prompt(&self) -> String {
        let mut lines = vec![
            "Review this shell command.".to_string(),
            String::new(),
            "<command>".to_string(),
            self.command.clone(),
            "</command>".to_string(),
            String::new(),
            "<execution_context>".to_string(),
            format!("cwd: {}", self.cwd.as_deref().unwrap_or("<default>")),
            format!("background: {}", py_bool(self.background)),
            format!(
                "environment_keys: {}",
                py_string_list(&self.environment_keys)
            ),
        ];
        if let Some(snapshot) = self.context_snapshot.as_ref() {
            lines.extend([
                format!(
                    "timeout_seconds: {}",
                    py_option_u64(snapshot.timeout_seconds)
                ),
                format!(
                    "tool_call_approved: {}",
                    py_bool(snapshot.tool_call_approved)
                ),
            ]);
        }
        lines.extend(["</execution_context>".to_string(), String::new()]);

        if let Some(snapshot) = self.context_snapshot.as_ref() {
            lines.extend([
                "<workspace_context>".to_string(),
                format!(
                    "default_cwd: {}",
                    snapshot.default_cwd.as_deref().unwrap_or("<unknown>")
                ),
                format!("allowed_paths: {}", py_string_list(&snapshot.allowed_paths)),
                format!(
                    "shell_platform: {}",
                    snapshot.shell_platform.as_deref().unwrap_or("<unknown>")
                ),
                format!(
                    "shell_executable: {}",
                    snapshot.shell_executable.as_deref().unwrap_or("<unknown>")
                ),
                "</workspace_context>".to_string(),
                String::new(),
            ]);
        }

        if !self.previous_reviews.is_empty() {
            lines.push("<previous_shell_reviews>".to_string());
            for (index, review) in self.previous_reviews.iter().enumerate() {
                lines.extend([
                    format!("review_{}:", index + 1),
                    format!("  approved: {}", py_bool(review.approved)),
                    format!("  risk_level: {}", risk_level_name(review.risk_level)),
                    format!("  reason: {}", review.reason),
                    format!(
                        "  command: {}",
                        review.command.as_deref().unwrap_or("<unknown>")
                    ),
                    format!("  cwd: {}", review.cwd.as_deref().unwrap_or("<default>")),
                ]);
            }
            lines.extend(["</previous_shell_reviews>".to_string(), String::new()]);
        }

        lines.join("\n")
    }

    /// Build HITL metadata for this request and decision.
    #[must_use]
    pub fn to_approval_metadata(&self, decision: &ShellReviewDecision) -> Value {
        let mut metadata = Map::new();
        metadata.insert(
            "reviewer".to_string(),
            Value::String("shell_command_reviewer".to_string()),
        );
        metadata.insert("reason".to_string(), Value::String(decision.reason.clone()));
        metadata.insert(
            "risk_level".to_string(),
            Value::String(risk_level_name(decision.risk_level).to_string()),
        );
        metadata.insert("command".to_string(), Value::String(self.command.clone()));
        metadata.insert("cwd".to_string(), option_string_value(self.cwd.as_deref()));
        metadata.insert("background".to_string(), Value::Bool(self.background));
        if let Some(snapshot) = self.context_snapshot.as_ref() {
            metadata.insert("context".to_string(), snapshot.to_metadata());
        }
        if !self.previous_reviews.is_empty() {
            metadata.insert(
                "previous_shell_reviews".to_string(),
                serde_json::to_value(&self.previous_reviews)
                    .unwrap_or_else(|_| Value::Array(Vec::new())),
            );
        }
        Value::Object(metadata)
    }
}

const fn py_bool(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn py_option_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "None".to_string(), |value| value.to_string())
}

fn py_string_list(values: &[String]) -> String {
    format!(
        "[{}]",
        values
            .iter()
            .map(|value| format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn option_string_value(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_string()))
}
