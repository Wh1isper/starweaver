use std::sync::Arc;

use starweaver_model::OutputMode;

use super::{OutputFunction, OutputSchema, OutputValidator};

/// Shared reference to an output function.
pub type DynOutputFunction = Arc<dyn OutputFunction>;

/// Decomposed output policy fields.
pub type OutputPolicyParts = (
    Option<OutputSchema>,
    Vec<Arc<dyn OutputValidator>>,
    Vec<DynOutputFunction>,
    Option<usize>,
    Option<OutputMode>,
    Option<bool>,
    Option<bool>,
);

/// Complete output behavior for one agent.
#[derive(Clone, Default)]
pub struct OutputPolicy {
    schema: Option<OutputSchema>,
    validators: Vec<Arc<dyn OutputValidator>>,
    functions: Vec<DynOutputFunction>,
    retries: Option<usize>,
    mode: Option<OutputMode>,
    allow_text_output: Option<bool>,
    allow_image_output: Option<bool>,
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

    /// Create a structured output policy from a Rust type.
    #[must_use]
    pub fn typed<T>() -> Self
    where
        T: schemars::JsonSchema,
    {
        Self::structured(OutputSchema::typed::<T>())
    }

    /// Create a named structured output policy from a Rust type.
    #[must_use]
    pub fn typed_named<T>(name: impl Into<String>) -> Self
    where
        T: schemars::JsonSchema,
    {
        Self::structured(OutputSchema::typed_named::<T>(name))
    }

    /// Create a plain text output policy.
    #[must_use]
    pub fn text() -> Self {
        Self::new().with_mode(OutputMode::Text)
    }

    /// Create an auto-selected structured output policy.
    #[must_use]
    pub fn auto(schema: OutputSchema) -> Self {
        Self::structured(schema).with_mode(OutputMode::Auto)
    }

    /// Create a provider-native JSON schema output policy.
    #[must_use]
    pub fn native_json_schema(schema: OutputSchema) -> Self {
        Self::structured(schema).with_mode(OutputMode::NativeJsonSchema)
    }

    /// Create a provider-native JSON object output policy.
    #[must_use]
    pub fn native_json_object(schema: OutputSchema) -> Self {
        Self::structured(schema).with_mode(OutputMode::NativeJsonObject)
    }

    /// Create a tool-call structured output policy.
    #[must_use]
    pub fn tool(schema: OutputSchema) -> Self {
        Self::structured(schema).with_mode(OutputMode::Tool)
    }

    /// Create a tool-call structured output policy with text fallback.
    #[must_use]
    pub fn tool_or_text(schema: OutputSchema) -> Self {
        Self::structured(schema)
            .with_mode(OutputMode::ToolOrText)
            .allow_text_output(true)
    }

    /// Create a prompted structured output policy.
    #[must_use]
    pub fn prompted(schema: OutputSchema) -> Self {
        Self::structured(schema).with_mode(OutputMode::Prompted)
    }

    /// Create an image output policy.
    #[must_use]
    pub fn image() -> Self {
        Self::new()
            .with_mode(OutputMode::Image)
            .allow_image_output(true)
            .allow_text_output(false)
    }

    /// Attach a structured output schema.
    #[must_use]
    pub fn with_schema(mut self, schema: OutputSchema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Select output mode for request preparation.
    #[must_use]
    pub const fn with_mode(mut self, mode: OutputMode) -> Self {
        self.mode = Some(mode);
        self
    }

    /// Configure whether text output is allowed.
    #[must_use]
    pub const fn allow_text_output(mut self, allow: bool) -> Self {
        self.allow_text_output = Some(allow);
        self
    }

    /// Configure whether image output is allowed.
    #[must_use]
    pub const fn allow_image_output(mut self, allow: bool) -> Self {
        self.allow_image_output = Some(allow);
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

    /// Return configured output mode.
    #[must_use]
    pub const fn mode(&self) -> Option<OutputMode> {
        self.mode
    }

    /// Return configured text-output allowance.
    #[must_use]
    pub const fn text_output_allowed(&self) -> Option<bool> {
        self.allow_text_output
    }

    /// Return configured image-output allowance.
    #[must_use]
    pub const fn image_output_allowed(&self) -> Option<bool> {
        self.allow_image_output
    }

    pub(crate) fn into_parts(self) -> OutputPolicyParts {
        (
            self.schema,
            self.validators,
            self.functions,
            self.retries,
            self.mode,
            self.allow_text_output,
            self.allow_image_output,
        )
    }
}
