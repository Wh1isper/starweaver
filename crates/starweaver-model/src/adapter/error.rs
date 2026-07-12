use serde_json::Value;
use thiserror::Error;

/// Model adapter error.
#[derive(Debug, Error)]
pub enum ModelError {
    /// Canonical history cannot be mapped into a provider request.
    #[error("message mapping failed: {0}")]
    MessageMapping(String),
    /// Provider response cannot be parsed into canonical response.
    #[error("response parsing failed: {0}")]
    ResponseParsing(String),
    /// Transport failed.
    #[error("transport failed: {0}")]
    Transport(String),
    /// A real HTTP model request was blocked by the global test guard.
    #[error("real model request blocked for {url}")]
    RealModelRequestBlocked {
        /// Target request URL.
        url: String,
    },
    /// Provider returned a non-success status.
    #[error("provider status {status}: {body}")]
    ProviderStatus {
        /// HTTP status code.
        status: u16,
        /// Provider response body.
        body: Value,
        /// Whether retry policy may retry this status.
        retryable: bool,
    },
    /// Retry attempts were exhausted.
    #[error("retry attempts exhausted after {attempts} attempts: {source}")]
    RetryExhausted {
        /// Attempt count.
        attempts: u32,
        /// Last error.
        source: Box<Self>,
    },
    /// Request or stream was cancelled by the runtime.
    #[error("model request cancelled: {reason}")]
    Cancelled {
        /// Cancellation reason.
        reason: String,
    },
    /// Provider returned an unsupported response shape.
    #[error("unsupported provider response: {0}")]
    UnsupportedResponse(String),
}

impl ModelError {
    /// Return an error message safe for durable events and client-visible streams.
    ///
    /// Provider status bodies remain available on the typed error for retry and
    /// diagnostic classification, but are omitted because they can contain
    /// provider-echoed request content, account details, or credentials.
    #[must_use]
    pub fn public_message(&self) -> String {
        match self {
            Self::ProviderStatus { status, .. } => format!("provider status {status}"),
            Self::RetryExhausted { attempts, source } => format!(
                "retry attempts exhausted after {attempts} attempts: {}",
                source.public_message()
            ),
            Self::MessageMapping(_) => "model request could not be constructed".to_string(),
            Self::ResponseParsing(_) => "provider response could not be parsed".to_string(),
            Self::Transport(_) => "model transport failed".to_string(),
            Self::RealModelRequestBlocked { .. } => "real model request blocked".to_string(),
            Self::Cancelled { .. } => "model request cancelled".to_string(),
            Self::UnsupportedResponse(_) => "provider returned an unsupported response".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::ModelError;

    #[test]
    fn public_provider_status_message_omits_response_body() {
        let error = ModelError::ProviderStatus {
            status: 401,
            body: json!({
                "error": "unauthorized",
                "echoed_token": "provider-secret"
            }),
            retryable: false,
        };

        assert_eq!(error.public_message(), "provider status 401");
        assert!(!error.public_message().contains("provider-secret"));
        assert!(error.to_string().contains("provider-secret"));
    }

    #[test]
    fn public_messages_redact_all_free_form_diagnostics() {
        let secret = "provider-secret";
        let cases = [
            (
                ModelError::MessageMapping(secret.to_string()),
                "model request could not be constructed",
            ),
            (
                ModelError::ResponseParsing(secret.to_string()),
                "provider response could not be parsed",
            ),
            (
                ModelError::Transport(format!(
                    "Authorization: Bearer {secret}; https://example.test?api_key={secret}"
                )),
                "model transport failed",
            ),
            (
                ModelError::RealModelRequestBlocked {
                    url: format!("https://example.test?api_key={secret}"),
                },
                "real model request blocked",
            ),
            (
                ModelError::Cancelled {
                    reason: secret.to_string(),
                },
                "model request cancelled",
            ),
            (
                ModelError::UnsupportedResponse(secret.to_string()),
                "provider returned an unsupported response",
            ),
        ];

        for (error, expected) in cases {
            assert_eq!(error.public_message(), expected);
            assert!(!error.public_message().contains(secret));
        }
    }

    #[test]
    fn public_retry_exhausted_message_redacts_nested_provider_body() {
        let error = ModelError::RetryExhausted {
            attempts: 3,
            source: Box::new(ModelError::ProviderStatus {
                status: 429,
                body: json!({"secret": "provider-secret"}),
                retryable: true,
            }),
        };

        assert_eq!(
            error.public_message(),
            "retry attempts exhausted after 3 attempts: provider status 429"
        );
        assert!(!error.public_message().contains("provider-secret"));
    }
}
