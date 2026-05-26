//! Structured output schemas and validators for agent runs.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starweaver_model::ToolDefinition;
use thiserror::Error;

use crate::run::AgentRunState;

/// Structured output schema passed to the model and used for runtime parsing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputSchema {
    /// Output schema name.
    pub name: String,
    /// Output schema description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for the final output value.
    pub schema: Value,
    /// Whether schema validation should be strict when provider support exists.
    #[serde(default)]
    pub strict: bool,
}

impl OutputSchema {
    /// Build a named output schema.
    #[must_use]
    pub fn new(name: impl Into<String>, schema: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            schema,
            strict: true,
        }
    }

    /// Add a description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set strict provider validation preference.
    #[must_use]
    pub const fn with_strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Return provider-neutral request schema metadata.
    #[must_use]
    pub fn request_schema(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "schema": self.schema,
            "strict": self.strict,
        })
    }
}

/// Parsed final output value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum OutputValue {
    /// Plain text output.
    Text(String),
    /// Structured JSON output.
    Json(Value),
}

impl OutputValue {
    /// Return text output, serializing JSON when needed.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Json(value) => value.to_string(),
        }
    }

    /// Return JSON output when this value is structured.
    #[must_use]
    pub const fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Text(_) => None,
            Self::Json(value) => Some(value),
        }
    }

    /// Parse this output value into a Rust type.
    ///
    /// # Errors
    ///
    /// Returns an error when text is not valid JSON or deserialization fails.
    pub fn parse<T>(&self) -> OutputValidationResult<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let value = match self {
            Self::Text(text) => serde_json::from_str(text)
                .map_err(|error| OutputValidationError::InvalidJson(error.to_string()))?,
            Self::Json(value) => value.clone(),
        };
        serde_json::from_value(value)
            .map_err(|error| OutputValidationError::Schema(error.to_string()))
    }
}

/// Shared reference to an output function.
pub type DynOutputFunction = Arc<dyn OutputFunction>;

/// Function-style final output call definition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OutputFunctionDefinition {
    /// Output function name exposed to the model.
    pub name: String,
    /// Output function description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON schema for output function arguments.
    pub parameters: Value,
}

impl OutputFunctionDefinition {
    /// Build an output function definition.
    #[must_use]
    pub fn new(name: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: None,
            parameters,
        }
    }

    /// Add description.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Convert to a provider-neutral tool definition.
    #[must_use]
    pub fn tool_definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
            metadata: serde_json::Map::new(),
        }
    }
}

/// Output function execution context.
#[derive(Clone, Debug)]
pub struct OutputFunctionContext {
    /// Run state at finalization time.
    pub state: AgentRunState,
}

/// Output function called by the model to finish a run.
#[async_trait]
pub trait OutputFunction: Send + Sync {
    /// Output function definition exposed to the model.
    fn definition(&self) -> OutputFunctionDefinition;

    /// Execute the output function with model-provided arguments.
    ///
    /// # Errors
    ///
    /// Returns an error when the output function rejects the arguments or fails.
    async fn call(
        &self,
        context: OutputFunctionContext,
        arguments: Value,
    ) -> OutputValidationResult<OutputValue>;
}

/// Function-backed output function.
pub struct FunctionOutputFunction<F> {
    definition: OutputFunctionDefinition,
    function: F,
}

impl<F> FunctionOutputFunction<F> {
    /// Build a function-backed output function.
    #[must_use]
    pub const fn new(definition: OutputFunctionDefinition, function: F) -> Self {
        Self {
            definition,
            function,
        }
    }
}

#[async_trait]
impl<F, Fut> OutputFunction for FunctionOutputFunction<F>
where
    F: Send + Sync + Fn(OutputFunctionContext, Value) -> Fut,
    Fut: Send + std::future::Future<Output = OutputValidationResult<OutputValue>>,
{
    fn definition(&self) -> OutputFunctionDefinition {
        self.definition.clone()
    }

    async fn call(
        &self,
        context: OutputFunctionContext,
        arguments: Value,
    ) -> OutputValidationResult<OutputValue> {
        (self.function)(context, arguments).await
    }
}

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
