#![allow(missing_docs, clippy::unwrap_used)]

use std::sync::Arc;

use starweaver_agent::{AgentBuilder, ReinjectSystemPromptProcessor, TestModel};

#[tokio::test]
async fn facade_reexports_reinject_system_prompt_processor() {
    let result = AgentBuilder::new(Arc::new(TestModel::with_text("ok")))
        .instruction("System policy")
        .history_processor(Arc::new(ReinjectSystemPromptProcessor::new()))
        .build()
        .run("hello")
        .await
        .unwrap();

    assert_eq!(result.output, "ok");
}
