#![allow(clippy::expect_used, clippy::unwrap_used, missing_docs)]

use std::sync::Arc;

use clap::Parser;
use starweaver_cli::{Cli, ConfigResolver, LocalSessionStore};
use starweaver_session::SessionStore;

#[allow(dead_code)]
#[path = "../../starweaver-session/tests/support/session_store_contract.rs"]
mod contract;

#[tokio::test]
async fn local_adapter_satisfies_shared_session_store_contract() {
    let temp = tempfile::tempdir().expect("temporary CLI root");
    let cli = Cli::parse_from(["starweaver-cli", "diagnostics"]);
    let config = ConfigResolver::for_tests(temp.path())
        .resolve(&cli)
        .expect("resolve CLI config");
    let store: Arc<dyn SessionStore> =
        Arc::new(LocalSessionStore::new(config).expect("open CLI session-store adapter"));
    Box::pin(contract::assert_session_store_contract(
        store,
        "cli-adapter",
    ))
    .await;
}
