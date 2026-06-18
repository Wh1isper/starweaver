//! Starweaver display message protocol and AGUI projection helpers.

mod custom;
mod projector;
mod types;

pub use projector::DefaultDisplayMessageProjector;
pub use types::{
    DisplayMessage, DisplayMessageKind, DisplayMessageProjector, DisplayProjectionContext,
    DisplayVisibility,
};
