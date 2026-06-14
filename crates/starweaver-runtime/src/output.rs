//! Structured output schemas and validators for agent runs.

mod function;
mod policy;
mod types;
mod validation;

pub use function::{
    FunctionOutputFunction, OutputFunction, OutputFunctionContext, OutputFunctionDefinition,
};
pub use policy::{DynOutputFunction, OutputPolicy};
pub use types::{OutputSchema, OutputValue};
pub use validation::{
    parse_output, FunctionOutputValidator, OutputValidationError, OutputValidationResult,
    OutputValidator,
};
