//! Deterministic model adapters for agent tests.

mod function_model;
mod helpers;
mod scripted;

pub use function_model::{
    FunctionModel, FunctionModelFn, FunctionModelInfo, FunctionModelStreamFn,
};
pub use helpers::{latest_user_text, tool_call_response};
pub use scripted::TestModel;
