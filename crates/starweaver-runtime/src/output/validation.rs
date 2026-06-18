use async_trait::async_trait;
use thiserror::Error;

use crate::run::AgentRunState;

use super::{OutputSchema, OutputValue};

/// Structured output validation error.
#[derive(Debug, Error)]
pub enum OutputValidationError {
    /// Output could not be parsed as JSON.
    #[error("output is not valid JSON: {0}")]
    InvalidJson(String),
    /// Output did not match the schema foundation checks.
    #[error("output schema validation failed: {0}")]
    Schema(String),
    /// Custom validator rejected the output and requested a model retry.
    #[error("output retry requested: {0}")]
    Retry(String),
    /// Custom validator failed the run.
    #[error("output validation failed: {0}")]
    Failed(String),
}

impl OutputValidationError {
    /// Create a retry validation error.
    #[must_use]
    pub fn retry(message: impl Into<String>) -> Self {
        Self::Retry(message.into())
    }

    /// Create a failed validation error.
    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// Output validator result.
pub type OutputValidationResult<T> = Result<T, OutputValidationError>;

/// Validator for parsed structured output values.
#[async_trait]
pub trait OutputValidator: Send + Sync {
    /// Validate a parsed output value.
    ///
    /// # Errors
    ///
    /// Returns an error when validation fails or should trigger a model retry.
    async fn validate(
        &self,
        state: &mut AgentRunState,
        output: &OutputValue,
    ) -> OutputValidationResult<()>;
}

/// Function-backed output validator.
pub struct FunctionOutputValidator<F> {
    function: F,
}

impl<F> FunctionOutputValidator<F> {
    /// Build a validator from a function.
    #[must_use]
    pub const fn new(function: F) -> Self {
        Self { function }
    }
}

#[async_trait]
impl<F, Fut> OutputValidator for FunctionOutputValidator<F>
where
    F: Send + Sync + Fn(&mut AgentRunState, &OutputValue) -> Fut,
    Fut: Send + std::future::Future<Output = OutputValidationResult<()>>,
{
    async fn validate(
        &self,
        state: &mut AgentRunState,
        output: &OutputValue,
    ) -> OutputValidationResult<()> {
        (self.function)(state, output).await
    }
}

/// Parse and validate raw text according to an optional output schema.
///
/// # Errors
///
/// Returns an error when parsing or foundation schema checks fail.
pub fn parse_output(
    raw_output: &str,
    schema: Option<&OutputSchema>,
) -> OutputValidationResult<OutputValue> {
    match schema {
        Some(schema) => {
            let value = serde_json::from_str(raw_output)
                .map_err(|error| OutputValidationError::InvalidJson(error.to_string()))?;
            validate_json_value(&value, schema)?;
            Ok(OutputValue::Json(value))
        }
        None => Ok(OutputValue::Text(raw_output.to_string())),
    }
}

fn validate_json_value(
    value: &serde_json::Value,
    schema: &OutputSchema,
) -> OutputValidationResult<()> {
    let validator = jsonschema::validator_for(&schema.schema).map_err(|error| {
        OutputValidationError::Schema(format!("invalid output schema {}: {error}", schema.name))
    })?;
    validator.validate(value).map_err(|error| {
        let path = error.instance_path.as_str();
        let detail = error.masked().to_string();
        if path.is_empty() {
            OutputValidationError::Schema(detail)
        } else {
            OutputValidationError::Schema(format!("{path}: {detail}"))
        }
    })
}
