use starweaver_context::AgentContext;
use starweaver_core::ConversationId;
use starweaver_model::ModelMessage;

use crate::{
    agent::{Agent, AgentError, AgentResult},
    iteration::{AgentIterResult, AgentIterationTrace},
    stream::{AgentStreamRecord, AgentStreamResult},
};

impl Agent {
    /// Run the agent with a user prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run(&self, prompt: impl Into<String>) -> Result<AgentResult, AgentError> {
        self.run_with_history(prompt, Vec::new()).await
    }

    /// Run the agent and collect a compact iteration trace.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_iter(&self, prompt: impl Into<String>) -> Result<AgentIterResult, AgentError> {
        let mut events = Vec::new();
        let result = self.run_with_stream_events(prompt, &mut events).await?;
        let iterations = AgentIterationTrace::from_stream_records(&events);
        Ok(AgentIterResult {
            result,
            iterations,
            events,
        })
    }

    /// Run the agent and collect typed stream events emitted during execution.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_stream(
        &self,
        prompt: impl Into<String>,
    ) -> Result<AgentStreamResult, AgentError> {
        let mut events = Vec::new();
        let result = self.run_with_stream_events(prompt, &mut events).await?;
        Ok(AgentStreamResult { result, events })
    }

    /// Run the agent with prior history and collect a compact iteration trace.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_history_iter(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
    ) -> Result<AgentIterResult, AgentError> {
        let mut events = Vec::new();
        let result = self
            .run_with_history_and_stream_events(prompt, message_history, &mut events)
            .await?;
        let iterations = AgentIterationTrace::from_stream_records(&events);
        Ok(AgentIterResult {
            result,
            iterations,
            events,
        })
    }

    /// Run the agent with an explicit typed stream event collector.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_stream_events(
        &self,
        prompt: impl Into<String>,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_history_and_stream_events(prompt, Vec::new(), events)
            .await
    }

    /// Run the agent with prior history and collect typed stream events.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_history_and_stream_events(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        let mut context = context_from_history(message_history);
        self.run_with_context_and_stream_events(prompt, &mut context, events)
            .await
    }

    /// Run the agent with prior canonical message history.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_history(
        &self,
        prompt: impl Into<String>,
        message_history: Vec<ModelMessage>,
    ) -> Result<AgentResult, AgentError> {
        let mut context = context_from_history(message_history);
        self.run_with_context(prompt, &mut context).await
    }

    /// Run the agent using a lifecycle-wide context and collect a compact iteration trace.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_context_iter(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
    ) -> Result<AgentIterResult, AgentError> {
        let mut events = Vec::new();
        let result = self
            .run_with_context_and_stream_events(prompt, context, &mut events)
            .await?;
        let iterations = AgentIterationTrace::from_stream_records(&events);
        Ok(AgentIterResult {
            result,
            iterations,
            events,
        })
    }

    /// Run the agent using a lifecycle-wide context and typed stream event collector.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    pub async fn run_with_context_and_stream_events(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
        events: &mut Vec<AgentStreamRecord>,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_context_inner(prompt, context, Some(events))
            .await
    }

    /// Run the agent using a lifecycle-wide context.
    ///
    /// # Errors
    ///
    /// Returns an error when the model, capabilities, validation, tools, or runtime policy fails.
    #[allow(clippy::too_many_lines)]
    pub async fn run_with_context(
        &self,
        prompt: impl Into<String>,
        context: &mut AgentContext,
    ) -> Result<AgentResult, AgentError> {
        self.run_with_context_inner(prompt, context, None).await
    }
}

fn context_from_history(message_history: Vec<ModelMessage>) -> AgentContext {
    let conversation_id = latest_conversation_id(&message_history).unwrap_or_default();
    AgentContext {
        conversation_id,
        message_history,
        ..AgentContext::default()
    }
}

fn latest_conversation_id(message_history: &[ModelMessage]) -> Option<ConversationId> {
    message_history
        .iter()
        .rev()
        .find_map(|message| match message {
            ModelMessage::Request(request) => request.conversation_id.clone(),
            ModelMessage::Response(response) => response.conversation_id.clone(),
        })
}
