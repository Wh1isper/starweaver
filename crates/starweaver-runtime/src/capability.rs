//! Capability hooks and bundles for the bare agent runtime.

mod bundle;
mod error;
mod hooks;
mod ordering;
mod spec;

/// Built-in runtime capability id for canonical `AgentContext` instruction injection.
pub const RUNTIME_CONTEXT_CAPABILITY_ID: &str = "starweaver.runtime.context";

pub use bundle::{CapabilityBundle, StaticCapabilityBundle};
pub use error::{CapabilityError, CapabilityResult};
pub use hooks::AgentCapability;
pub use ordering::{resolve_capability_order, CapabilityOrderError};
pub use spec::{CapabilityId, CapabilityOrdering, CapabilitySpec, RetryEventKind};
