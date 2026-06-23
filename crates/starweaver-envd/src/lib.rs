//! Local envd service implementation.

mod convert;
mod local;
mod rpc;
mod transport;

pub use local::LocalEnvd;
pub use rpc::EnvdRpcService;
pub use transport::{run_http, run_stdio};

#[cfg(test)]
mod tests;
