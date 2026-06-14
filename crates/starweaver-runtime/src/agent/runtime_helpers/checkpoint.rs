//! Executor checkpoint helper.

use starweaver_context::AgentContext;

use crate::{
    agent::{Agent, AgentError},
    executor::{AgentCheckpoint, AgentExecutionDecision, AgentExecutionNode},
    run::AgentRunState,
    trace::{SpanSpec, SpanStatus},
};

impl Agent {
    pub(in crate::agent) async fn checkpoint(
        &self,
        node: AgentExecutionNode,
        state: &AgentRunState,
        context: &AgentContext,
    ) -> Result<AgentExecutionDecision, AgentError> {
        let checkpoint_span = self.trace_recorder.start_span(
            SpanSpec::new("starweaver.checkpoint")
                .with_attribute("starweaver.checkpoint.node", serde_json::json!(node)),
            &context.trace_context,
        );
        let mut checkpoint = AgentCheckpoint::new(node, state);
        checkpoint.resume.trace_context = checkpoint_span.context().clone();
        checkpoint.metadata.insert(
            "trace_id".to_string(),
            serde_json::json!(checkpoint_span.context().trace_id),
        );
        checkpoint.metadata.insert(
            "span_id".to_string(),
            serde_json::json!(checkpoint_span.context().span_id),
        );
        for capability in &self.ordered_capabilities()? {
            capability
                .on_checkpoint_with_context(state, context, &checkpoint)
                .await
                .map_err(Self::capability_error)?;
        }
        let decision = self.executor.checkpoint(checkpoint).await?;
        self.trace_recorder
            .close_span(&checkpoint_span, SpanStatus::Ok);
        Ok(decision)
    }
}
