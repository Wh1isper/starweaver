//! Provider transport progression for one prepared model request.

use starweaver_model::{
    ModelRequestContext, ModelResponse, ModelResponseEventStream, ModelResponseStreamEvent,
    ModelRunSession,
    transport::{RetryPolicy, should_retry_error},
};

use super::{DEFAULT_MODEL_ERROR_RETRIES, PreparedProviderRequest};

#[derive(Clone, Copy)]
pub(super) enum ProviderInvocationMode {
    Incremental,
    FinalOnly,
}

pub(super) enum ProviderStreamResumeCause {
    ModelError { message: String },
    MissingFinalResult,
}

impl ProviderStreamResumeCause {
    pub(super) fn public_message(&self) -> &str {
        match self {
            Self::ModelError { message } => message,
            Self::MissingFinalResult => "model stream ended before final result",
        }
    }
}

pub(super) struct ProviderStreamResume {
    pub(super) retry: usize,
    pub(super) max_retries: usize,
    pub(super) cause: ProviderStreamResumeCause,
}

pub(super) enum ProviderInvocationStep {
    StreamEvent(ModelResponseStreamEvent),
    StreamResume(ProviderStreamResume),
    StreamAttemptEnded,
    Complete(ModelResponse),
    ModelError(starweaver_model::ModelError),
    MissingFinalResult,
}

pub(super) struct ProviderInvocation {
    request: PreparedProviderRequest,
    request_context: ModelRequestContext,
    mode: ProviderInvocationMode,
    stream: Option<ModelResponseEventStream>,
    stream_resume_retries_used: usize,
    max_stream_resume_retries: usize,
}

impl ProviderInvocation {
    pub(super) const fn new(
        request: PreparedProviderRequest,
        request_context: ModelRequestContext,
        mode: ProviderInvocationMode,
    ) -> Self {
        Self {
            request,
            request_context,
            mode,
            stream: None,
            stream_resume_retries_used: 0,
            max_stream_resume_retries: DEFAULT_MODEL_ERROR_RETRIES,
        }
    }

    fn stream_resume(&mut self, cause: ProviderStreamResumeCause) -> Option<ProviderStreamResume> {
        if self.stream_resume_retries_used >= self.max_stream_resume_retries {
            return None;
        }
        self.stream_resume_retries_used = self.stream_resume_retries_used.saturating_add(1);
        Some(ProviderStreamResume {
            retry: self.stream_resume_retries_used,
            max_retries: self.max_stream_resume_retries,
            cause,
        })
    }

    pub(super) async fn next(
        &mut self,
        model_session: &mut dyn ModelRunSession,
    ) -> ProviderInvocationStep {
        match self.mode {
            ProviderInvocationMode::FinalOnly => {
                match model_session
                    .request_stream_final(
                        self.request.messages.clone(),
                        self.request.settings.clone(),
                        self.request.params.clone(),
                        self.request_context.clone(),
                    )
                    .await
                {
                    Ok(response) => ProviderInvocationStep::Complete(response),
                    Err(error) => ProviderInvocationStep::ModelError(error),
                }
            }
            ProviderInvocationMode::Incremental => {
                if self.stream.is_none() {
                    match model_session
                        .request_stream_incremental(
                            self.request.messages.clone(),
                            self.request.settings.clone(),
                            self.request.params.clone(),
                            self.request_context.clone(),
                        )
                        .await
                    {
                        Ok(stream) => self.stream = Some(stream),
                        Err(error) => {
                            if should_resume_provider_stream(&error)
                                && let Some(resume) =
                                    self.stream_resume(ProviderStreamResumeCause::ModelError {
                                        message: error.public_message(),
                                    })
                            {
                                return ProviderInvocationStep::StreamResume(resume);
                            }
                            return ProviderInvocationStep::ModelError(error);
                        }
                    }
                }
                let Some(stream) = self.stream.as_mut() else {
                    unreachable!("incremental provider stream must be initialized");
                };
                match stream.recv().await {
                    Some(Ok(event)) => ProviderInvocationStep::StreamEvent(event),
                    Some(Err(error)) => {
                        self.stream = None;
                        if should_resume_provider_stream(&error)
                            && let Some(resume) =
                                self.stream_resume(ProviderStreamResumeCause::ModelError {
                                    message: error.public_message(),
                                })
                        {
                            return ProviderInvocationStep::StreamResume(resume);
                        }
                        ProviderInvocationStep::ModelError(error)
                    }
                    None => {
                        self.stream = None;
                        ProviderInvocationStep::StreamAttemptEnded
                    }
                }
            }
        }
    }

    pub(super) fn finish_stream_attempt(
        &mut self,
        response: Option<ModelResponse>,
    ) -> ProviderInvocationStep {
        if let Some(response) = response {
            return ProviderInvocationStep::Complete(response);
        }
        self.stream_resume(ProviderStreamResumeCause::MissingFinalResult)
            .map_or(
                ProviderInvocationStep::MissingFinalResult,
                ProviderInvocationStep::StreamResume,
            )
    }
}

fn should_resume_provider_stream(error: &starweaver_model::ModelError) -> bool {
    should_retry_error(error, &RetryPolicy::default())
}
