#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_model::TestModel;
use starweaver_runtime::{Agent, AgentIterationKind};

#[tokio::test]
async fn run_iter_returns_compact_iteration_trace() {
    let iter = Agent::new(Arc::new(TestModel::with_text("ok")))
        .run_iter("hello")
        .await
        .unwrap();

    assert_eq!(iter.result.output, "ok");
    assert!(iter.iterations.is_complete());
    assert!(
        iter.iterations
            .steps()
            .iter()
            .any(|step| step.kind == AgentIterationKind::ModelRequest)
    );
    assert!(
        iter.iterations
            .steps()
            .iter()
            .any(|step| step.kind == AgentIterationKind::Checkpoint)
    );
    assert_eq!(iter.iterations.steps()[0].stream_sequence, 0);
}

#[tokio::test]
async fn run_with_history_iter_preserves_new_messages() {
    let first = Agent::new(Arc::new(TestModel::with_text("first")))
        .run("hello")
        .await
        .unwrap();
    let iter = Agent::new(Arc::new(TestModel::with_text("second")))
        .run_with_history_iter("again", first.messages)
        .await
        .unwrap();

    assert_eq!(iter.result.output, "second");
    assert_eq!(iter.result.new_messages().len(), 2);
    assert!(iter.iterations.is_complete());
}
