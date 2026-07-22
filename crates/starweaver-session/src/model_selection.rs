//! Product-neutral durable model-selection and mutation-receipt contracts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{PendingHostEventPublication, SessionStoreError, SessionStoreResult};

/// Stable operation identifier recorded for model-selection mutations.
pub const MODEL_SELECTION_OPERATION: &str = "model_selection.select";

/// Authority-bound durable model selection.
///
/// `authority_binding` is a trusted host-derived binding (for example an authorization-scope
/// fingerprint), not a caller-selected display name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableModelSelection {
    /// Stable authority binding that owns this selection.
    pub authority_binding: String,
    /// Selected profile identifier.
    pub selected_profile: String,
    /// Effective model identifier resolved by the caller's catalog.
    pub model_id: String,
    /// Monotonic selection revision, beginning at one.
    pub revision: u64,
    /// Time at which this revision was committed.
    pub updated_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for DurableModelSelection {
    const SCHEMA: &'static str = "starweaver.session.model_selection";
}

impl DurableModelSelection {
    /// Validate durable model-selection invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when an identity is empty or the revision is zero.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("model selection authority binding", &self.authority_binding)?;
        require_non_empty("model selection profile", &self.selected_profile)?;
        require_non_empty("model selection model id", &self.model_id)?;
        if self.revision == 0 {
            return Err(SessionStoreError::Failed(
                "model selection revision must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

/// Caller-supplied default used only when an authority has no durable selection yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InitializeModelSelection {
    /// Stable trusted authority binding.
    pub authority_binding: String,
    /// Default selected profile.
    pub selected_profile: String,
    /// Effective model for the default profile.
    pub model_id: String,
    /// Initialization timestamp persisted with revision one.
    pub initialized_at: DateTime<Utc>,
}

impl InitializeModelSelection {
    /// Validate initialization input.
    ///
    /// # Errors
    ///
    /// Returns an error when a required string is empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("model selection authority binding", &self.authority_binding)?;
        require_non_empty("model selection profile", &self.selected_profile)?;
        require_non_empty("model selection model id", &self.model_id)
    }
}

/// Durable, wire-independent projection of an idempotent mutation receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MutationReceipt {
    /// Stable receipt identity.
    pub receipt_id: String,
    /// Idempotency key bound to the canonical fingerprint.
    pub idempotency_key: String,
    /// Canonical normalized command fingerprint.
    pub fingerprint: String,
    /// Product-neutral operation identifier.
    pub operation: String,
    /// Durable mutation outcome.
    pub state: String,
    /// Stable target binding.
    pub target_ref: String,
    /// Whether external reconciliation is required.
    pub reconciliation_required: bool,
    /// Whether this returned projection came from an exact replay.
    ///
    /// The durable original is always stored with `false`; storage sets this to `true` only on a
    /// cloned return projection for an exact idempotent replay.
    pub replayed: bool,
    /// Time at which the original mutation committed.
    pub created_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for MutationReceipt {
    const SCHEMA: &'static str = "starweaver.session.mutation_receipt";
}

impl MutationReceipt {
    /// Return a replay projection without mutating the durable original receipt.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        let mut projection = self.clone();
        projection.replayed = true;
        projection
    }

    /// Validate mutation-receipt invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when a required identity is empty.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("mutation receipt id", &self.receipt_id)?;
        require_non_empty("mutation receipt idempotency key", &self.idempotency_key)?;
        require_non_empty("mutation receipt fingerprint", &self.fingerprint)?;
        require_non_empty("mutation receipt operation", &self.operation)?;
        require_non_empty("mutation receipt state", &self.state)?;
        require_non_empty("mutation receipt target", &self.target_ref)
    }
}

/// Atomic model-selection mutation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectModel {
    /// Stable trusted authority binding that owns the state and idempotency namespace.
    pub authority_binding: String,
    /// Profile to select.
    pub selected_profile: String,
    /// Effective model identifier resolved by the caller's catalog.
    pub model_id: String,
    /// Idempotency key scoped to `authority_binding`.
    pub idempotency_key: String,
    /// Canonical fingerprint of the normalized authorized command.
    pub command_fingerprint: String,
    /// Authoritative mutation timestamp.
    pub occurred_at: DateTime<Utc>,
    /// Optional product-neutral durable host event to enqueue in the same transaction.
    pub host_event_publication: Option<PendingHostEventPublication>,
}

impl SelectModel {
    /// Validate mutation input.
    ///
    /// # Errors
    ///
    /// Returns an error when required strings or optional event evidence are invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        require_non_empty("model selection authority binding", &self.authority_binding)?;
        require_non_empty("model selection profile", &self.selected_profile)?;
        require_non_empty("model selection model id", &self.model_id)?;
        require_non_empty("model selection idempotency key", &self.idempotency_key)?;
        require_non_empty(
            "model selection command fingerprint",
            &self.command_fingerprint,
        )?;
        if let Some(publication) = &self.host_event_publication {
            publication.validate()?;
        }
        Ok(())
    }
}

/// Durable result of one model-selection mutation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelSelectionMutationReceipt {
    /// Selection committed by the original mutation.
    pub selection: DurableModelSelection,
    /// Durable mutation receipt, projected with `replayed = true` for exact retries.
    pub receipt: MutationReceipt,
}

impl starweaver_core::VersionedRecord for ModelSelectionMutationReceipt {
    const SCHEMA: &'static str = "starweaver.session.model_selection_mutation_receipt";
}

impl ModelSelectionMutationReceipt {
    /// Return an exact-replay projection while preserving the durable original value.
    #[must_use]
    pub fn replayed_projection(&self) -> Self {
        Self {
            selection: self.selection.clone(),
            receipt: self.receipt.replayed_projection(),
        }
    }

    /// Validate persisted result invariants.
    ///
    /// # Errors
    ///
    /// Returns an error when selection and receipt identities disagree or are invalid.
    pub fn validate(&self) -> SessionStoreResult<()> {
        self.selection.validate()?;
        self.receipt.validate()?;
        if self.receipt.target_ref != self.selection.authority_binding {
            return Err(SessionStoreError::Conflict(
                "model selection receipt target does not match selection authority".to_string(),
            ));
        }
        if self.receipt.operation != MODEL_SELECTION_OPERATION {
            return Err(SessionStoreError::Conflict(
                "model selection receipt operation is invalid".to_string(),
            ));
        }
        Ok(())
    }
}

fn require_non_empty(label: &str, value: &str) -> SessionStoreResult<()> {
    if value.is_empty() {
        return Err(SessionStoreError::Failed(format!(
            "{label} cannot be empty"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{DurableModelSelection, ModelSelectionMutationReceipt, MutationReceipt};

    #[test]
    fn replay_projection_does_not_mutate_durable_original() {
        let original = ModelSelectionMutationReceipt {
            selection: DurableModelSelection {
                authority_binding: "authority-a".to_string(),
                selected_profile: "coding".to_string(),
                model_id: "model-a".to_string(),
                revision: 2,
                updated_at: Utc::now(),
            },
            receipt: MutationReceipt {
                receipt_id: "receipt-a".to_string(),
                idempotency_key: "key-a".to_string(),
                fingerprint: "sha256:a".to_string(),
                operation: super::MODEL_SELECTION_OPERATION.to_string(),
                state: "applied".to_string(),
                target_ref: "authority-a".to_string(),
                reconciliation_required: false,
                replayed: false,
                created_at: Utc::now(),
            },
        };

        let replay = original.replayed_projection();
        assert!(replay.receipt.replayed);
        assert!(!original.receipt.replayed);
        assert_eq!(replay.selection, original.selection);
    }
}
