//! Structured output schemas and validators for agent runs.

mod function;
mod policy;
mod types;
mod validation;

pub use function::{
    FunctionOutputFunction, OutputFunction, OutputFunctionContext, OutputFunctionDefinition,
    SchemaOutputFunction,
};
pub use policy::{DynOutputFunction, OutputPolicy};
pub use types::{OutputMedia, OutputSchema, OutputValue};
pub use validation::{
    FunctionOutputValidator, OutputValidationError, OutputValidationResult, OutputValidator,
    parse_output,
};
