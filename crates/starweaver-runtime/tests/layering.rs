//! Runtime layering tests.

use std::sync::Arc;

use starweaver_model::TestModel;
use starweaver_runtime::Agent;

#[tokio::test]
async fn runtime_agent_stays_focused_on_core_loop() {
    let result = Agent::new(Arc::new(TestModel::with_text("core")))
        .run("hello")
        .await;

    assert!(result.is_ok());
}
