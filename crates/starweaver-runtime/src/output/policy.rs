use std::sync::Arc;

use super::{OutputFunction, OutputSchema, OutputValidator};

/// Shared reference to an output function.
pub type DynOutputFunction = Arc<dyn OutputFunction>;

/// Decomposed output policy fields.
pub type OutputPolicyParts = (
    Option<OutputSchema>,
    Vec<Arc<dyn OutputValidator>>,
    Vec<DynOutputFunction>,
    Option<usize>,
);

/// Complete output behavior for one agent.
#[derive(Clone, Default)]
pub struct OutputPolicy {
    schema: Option<OutputSchema>,
    validators: Vec<Arc<dyn OutputValidator>>,
    functions: Vec<DynOutputFunction>,
    retries: Option<usize>,
}

impl OutputPolicy {
    /// Create an empty output policy.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a policy with a structured output schema.
    #[must_use]
    pub fn structured(schema: OutputSchema) -> Self {
        Self::new().with_schema(schema)
    }

    /// Attach a structured output schema.
    #[must_use]
    pub fn with_schema(mut self, schema: OutputSchema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Attach a validator.
    #[must_use]
    pub fn with_validator(mut self, validator: Arc<dyn OutputValidator>) -> Self {
        self.validators.push(validator);
        self
    }

    /// Attach an output function.
    #[must_use]
    pub fn with_function(mut self, function: DynOutputFunction) -> Self {
        self.functions.push(function);
        self
    }

    /// Set output retry budget.
    #[must_use]
    pub const fn with_retries(mut self, retries: usize) -> Self {
        self.retries = Some(retries);
        self
    }

    /// Return configured schema.
    #[must_use]
    pub const fn schema(&self) -> Option<&OutputSchema> {
        self.schema.as_ref()
    }

    /// Return configured validators.
    #[must_use]
    pub fn validators(&self) -> &[Arc<dyn OutputValidator>] {
        &self.validators
    }

    /// Return configured output functions.
    #[must_use]
    pub fn functions(&self) -> &[DynOutputFunction] {
        &self.functions
    }

    /// Return configured retry budget.
    #[must_use]
    pub const fn retries(&self) -> Option<usize> {
        self.retries
    }

    pub(crate) fn into_parts(self) -> OutputPolicyParts {
        (self.schema, self.validators, self.functions, self.retries)
    }
}
