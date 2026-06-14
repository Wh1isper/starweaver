use async_trait::async_trait;
use serde_json::Value;
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
            validate_json_value(&value, &schema.schema)?;
            Ok(OutputValue::Json(value))
        }
        None => Ok(OutputValue::Text(raw_output.to_string())),
    }
}

fn validate_json_value(value: &Value, schema: &Value) -> OutputValidationResult<()> {
    if let Some(schema_type) = schema.get("type").and_then(Value::as_str) {
        validate_type(value, schema_type)?;
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        validate_required(value, required)?;
    }
    Ok(())
}

fn validate_type(value: &Value, schema_type: &str) -> OutputValidationResult<()> {
    let valid = match schema_type {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    };
    if valid {
        Ok(())
    } else {
        Err(OutputValidationError::Schema(format!(
            "expected JSON value of type {schema_type}"
        )))
    }
}

fn validate_required(value: &Value, required: &[Value]) -> OutputValidationResult<()> {
    let object = value.as_object().ok_or_else(|| {
        OutputValidationError::Schema("required fields need an object output".to_string())
    })?;
    for field in required.iter().filter_map(Value::as_str) {
        if !object.contains_key(field) {
            return Err(OutputValidationError::Schema(format!(
                "missing required field {field}"
            )));
        }
    }
    Ok(())
}
