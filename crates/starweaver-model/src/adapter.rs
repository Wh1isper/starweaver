//! Model adapter traits and request context types.

mod context;
mod error;
mod guard;
mod params;
mod stream;
mod traits;

pub use context::ModelRequestContext;
pub use error::ModelError;
pub use guard::{
    allow_real_model_requests, allow_real_model_requests_guard, block_real_model_requests,
    set_allow_real_model_requests, RealModelRequestGuard,
};
pub use params::{ModelRequestParameters, NativeToolDefinition, ToolDefinition};
pub use stream::ModelResponseEventStream;
pub use traits::ModelAdapter;
