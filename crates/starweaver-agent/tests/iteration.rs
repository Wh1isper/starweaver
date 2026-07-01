#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentIterationKind, TestModel};

#[tokio::test]
async fn sdk_session_exposes_run_iter() {
    let app = AgentBuilder::new(Arc::new(TestModel::with_text("ok"))).build_app();
    let mut session = app.session();

    let result = session.run_iter("hello").await.unwrap();

    assert_eq!(result.result.output, "ok");
    assert!(result.iterations.is_complete());
    assert!(
        result
            .iterations
            .steps()
            .iter()
            .any(|step| step.kind == AgentIterationKind::RunComplete)
    );
}
