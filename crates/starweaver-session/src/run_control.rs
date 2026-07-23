//! Product-neutral durable live-run control admission contracts.
//!
//! A control request is first admitted as durable intent under the run's active fencing lease.
//! Delivery to a process-local runtime happens only after that transaction commits. Runtime
//! delivery is keyed by [`DurableRunControlIntent::operation_id`], allowing an exact retry after
//! an ambiguous failure window without injecting a second steering message or interrupt.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{DurableControlReceipt, ManagedRunTarget, RunAdmissionLease};

/// Durable control effect carried by an admitted intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DurableRunControlEffect {
    /// Inject one user steering message at the next runtime control-drain boundary.
    Steer {
        /// Model-visible steering text.
        text: String,
    },
    /// Request cooperative interruption of the active runtime.
    Interrupt {
        /// Product-neutral, safe reason category or message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl DurableRunControlEffect {
    /// Stable operation category used by receipts and persistence indexes.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self {
            Self::Steer { .. } => "steer",
            Self::Interrupt { .. } => "interrupt",
        }
    }
}

/// Durable lifecycle of one admitted control effect.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableRunControlStatus {
    /// Receipt and effect intent committed atomically; no runtime acceptance is yet recorded.
    Pending,
    /// The operation-id-aware runtime accepted the effect.
    Delivered,
    /// The runtime observed/consumed the effect at a control boundary.
    Consumed,
    /// The effect was made irrelevant by terminal state, stale fencing, or recovery policy.
    Reconciled,
}

impl DurableRunControlStatus {
    /// Stable storage/receipt state name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Consumed => "consumed",
            Self::Reconciled => "reconciled",
        }
    }

    /// Return whether `next` is a valid monotonic transition.
    #[must_use]
    pub const fn can_advance_to(self, next: Self) -> bool {
        self as u8 == next as u8
            || matches!(
                (self, next),
                (Self::Pending, Self::Delivered | Self::Reconciled)
                    | (Self::Delivered, Self::Consumed | Self::Reconciled)
                    | (Self::Consumed, Self::Reconciled)
            )
    }
}

/// Request to atomically reserve a receipt and its durable control effect.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AdmitRunControl {
    /// Active run authority and fencing lease. Stores validate it in the admission transaction.
    pub lease: RunAdmissionLease,
    /// Stable product authority identity. Idempotency keys are scoped to this binding.
    pub authority_binding: String,
    /// Deterministic operation identity used for runtime deduplication.
    pub operation_id: String,
    /// Deterministic durable receipt identity.
    pub receipt_id: String,
    /// Authority-scoped idempotency key.
    pub idempotency_key: String,
    /// Canonical command fingerprint bound to the key and authority.
    pub command_fingerprint: String,
    /// Durable effect payload.
    pub effect: DurableRunControlEffect,
    /// Caller-selected admission time retained across exact retries.
    pub created_at: DateTime<Utc>,
}

impl AdmitRunControl {
    /// Convert an admission request into its initial durable intent.
    #[must_use]
    pub fn into_intent(self) -> DurableRunControlIntent {
        let receipt = DurableControlReceipt {
            receipt_id: self.receipt_id,
            target: self.lease.target.clone(),
            operation_id: self.operation_id.clone(),
            operation: self.effect.operation().to_string(),
            idempotency_key: self.idempotency_key,
            command_fingerprint: self.command_fingerprint,
            fencing_generation: self.lease.fencing_generation,
            state: DurableRunControlStatus::Pending.as_str().to_string(),
            created_at: self.created_at,
        };
        DurableRunControlIntent {
            operation_id: self.operation_id,
            target: self.lease.target,
            authority_binding: self.authority_binding,
            admission_id: self.lease.admission_id,
            host_instance_id: self.lease.host_instance_id,
            fencing_generation: self.lease.fencing_generation,
            idempotency_key: receipt.idempotency_key.clone(),
            command_fingerprint: receipt.command_fingerprint.clone(),
            receipt,
            effect: self.effect,
            status: DurableRunControlStatus::Pending,
            created_at: self.created_at,
            delivered_at: None,
            consumed_at: None,
            reconciled_at: None,
        }
    }
}

/// Receipt plus durable steering inbox entry or interrupt intent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DurableRunControlIntent {
    /// Runtime deduplication identity.
    pub operation_id: String,
    /// Composite run target.
    pub target: ManagedRunTarget,
    /// Product authority binding.
    pub authority_binding: String,
    /// Admission identity that owned the run when the effect was accepted.
    pub admission_id: String,
    /// Host authority that owned the admission.
    pub host_instance_id: String,
    /// Monotonic fencing generation.
    pub fencing_generation: u64,
    /// Authority-scoped idempotency key.
    pub idempotency_key: String,
    /// Canonical command fingerprint.
    pub command_fingerprint: String,
    /// Durable public receipt reserved in the same transaction.
    pub receipt: DurableControlReceipt,
    /// Steering inbox entry or interrupt intent.
    pub effect: DurableRunControlEffect,
    /// Current durable effect state.
    pub status: DurableRunControlStatus,
    /// Admission time.
    pub created_at: DateTime<Utc>,
    /// First durable runtime-delivery acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<DateTime<Utc>>,
    /// First durable runtime-consumption acknowledgement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at: Option<DateTime<Utc>>,
    /// Recovery reconciliation time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconciled_at: Option<DateTime<Utc>>,
}

impl starweaver_core::VersionedRecord for DurableRunControlIntent {
    const SCHEMA: &'static str = "starweaver.session.durable_run_control_intent";
}

impl DurableRunControlIntent {
    /// Return whether this intent has exactly the admission identity supplied by a retry.
    #[must_use]
    pub fn matches_admission(&self, request: &AdmitRunControl) -> bool {
        self.target == request.lease.target
            && self.authority_binding == request.authority_binding
            && self.admission_id == request.lease.admission_id
            && self.host_instance_id == request.lease.host_instance_id
            && self.fencing_generation == request.lease.fencing_generation
            && self.operation_id == request.operation_id
            && self.receipt.receipt_id == request.receipt_id
            && self.idempotency_key == request.idempotency_key
            && self.command_fingerprint == request.command_fingerprint
            && self.effect == request.effect
    }

    /// Advance the state and synchronize the compatibility receipt projection.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-monotonic state transition.
    pub fn advance(
        &mut self,
        next: DurableRunControlStatus,
        occurred_at: DateTime<Utc>,
    ) -> Result<(), &'static str> {
        if !self.status.can_advance_to(next) {
            return Err("invalid durable run control state transition");
        }
        if self.status == next {
            return Ok(());
        }
        self.status = next;
        self.receipt.state = next.as_str().to_string();
        match next {
            DurableRunControlStatus::Pending => {}
            DurableRunControlStatus::Delivered => self.delivered_at = Some(occurred_at),
            DurableRunControlStatus::Consumed => {
                self.delivered_at.get_or_insert(occurred_at);
                self.consumed_at = Some(occurred_at);
            }
            DurableRunControlStatus::Reconciled => self.reconciled_at = Some(occurred_at),
        }
        Ok(())
    }
}

/// Derive a deterministic operation id from all authority/idempotency identity components.
#[must_use]
pub fn deterministic_run_control_operation_id(
    operation: &str,
    authority_binding: &str,
    target: &ManagedRunTarget,
    idempotency_key: &str,
    command_fingerprint: &str,
) -> String {
    let mut digest = Sha256::new();
    for component in [
        "starweaver.session.run_control.operation.v1",
        operation,
        authority_binding,
        target.namespace_id.as_str(),
        target.session_id.as_str(),
        target.run_id.as_str(),
        idempotency_key,
        command_fingerprint,
    ] {
        digest.update(component.len().to_be_bytes());
        digest.update(component.as_bytes());
    }
    format!("run_control_{:x}", digest.finalize())
}

/// Derive a deterministic receipt id from the operation identity.
#[must_use]
pub fn deterministic_run_control_receipt_id(operation_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"starweaver.session.run_control.receipt.v1\0");
    digest.update(operation_id.as_bytes());
    format!("control_{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use starweaver_core::{RunId, SessionId};

    #[test]
    fn operation_identity_binds_authority_target_key_and_fingerprint() {
        let target = ManagedRunTarget::new(
            "local",
            SessionId::from_string("session-a"),
            RunId::from_string("run-a"),
        );
        let first = deterministic_run_control_operation_id(
            "steer",
            "authority-a",
            &target,
            "key-a",
            "sha256:a",
        );
        assert_eq!(
            first,
            deterministic_run_control_operation_id(
                "steer",
                "authority-a",
                &target,
                "key-a",
                "sha256:a"
            )
        );
        assert_ne!(
            first,
            deterministic_run_control_operation_id(
                "steer",
                "authority-b",
                &target,
                "key-a",
                "sha256:a"
            )
        );
        assert_ne!(
            first,
            deterministic_run_control_operation_id(
                "interrupt",
                "authority-a",
                &target,
                "key-a",
                "sha256:a"
            )
        );
    }

    #[test]
    fn control_states_are_monotonic() {
        assert!(
            DurableRunControlStatus::Pending.can_advance_to(DurableRunControlStatus::Delivered)
        );
        assert!(
            DurableRunControlStatus::Delivered.can_advance_to(DurableRunControlStatus::Consumed)
        );
        assert!(
            DurableRunControlStatus::Consumed.can_advance_to(DurableRunControlStatus::Reconciled)
        );
        assert!(
            !DurableRunControlStatus::Consumed.can_advance_to(DurableRunControlStatus::Pending)
        );
    }
}
