//! Reusable model adapter wrappers.

mod concurrency;
mod fallback;
mod hooked;
mod profile_override;

use std::sync::Arc;

use crate::adapter::ModelAdapter;

pub use concurrency::ConcurrencyLimitedModel;
pub use fallback::FallbackModel;
pub use hooked::{DynModelExecutionHook, HookedModel, ModelExecutionHook, ModelExecutionMetadata};
pub use profile_override::ProfileOverrideModel;

/// Shared model adapter reference used by wrappers.
pub type DynModelAdapter = Arc<dyn ModelAdapter>;
