#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, AgentStreamEvent, TestModel};

#[tokio::test]
async fn facade_reexports_stream_event_types() {
    let stream = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
        .build()
        .run_stream("hello")
        .await
        .unwrap();

    assert_eq!(stream.result().output, "ok");
    assert!(matches!(
        stream.events()[0].event,
        AgentStreamEvent::RunStart { .. }
    ));
    assert!(matches!(
        stream.events().last().unwrap().event,
        AgentStreamEvent::RunComplete { .. }
    ));
}
