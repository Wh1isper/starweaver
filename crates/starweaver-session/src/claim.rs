//! Durable ownership claims for HITL continuation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use starweaver_core::{RunId, SessionId};

/// Durable metadata key carrying a typed continuation-effect recovery projection.
pub const CONTINUATION_EFFECT_METADATA_KEY: &str = "starweaver.continuation.effect";

/// Durable resume-claim phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitlResumeClaimState {
    /// Ownership is acquired but no continuation admission exists yet.
    Preflight,
    /// A fenced continuation run was admitted, but no hook or tool may execute yet.
    Admitted,
    /// Continuation execution may have produced external effects; release is forbidden.
    Started,
}

/// Result of atomically aborting a fenced HITL replacement before worker launch.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitlResumeAbortOutcome {
    /// The claim was still admitted, so no approved effect could have run and the source remains waiting.
    AbortedBeforeEffect,
    /// The claim was already started; callers must persist fail-closed related-run evidence instead.
    EffectStarted,
}

/// Durable effect boundary reached by a continuation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationEffectPhase {
    /// The effect fence was crossed and an approved hook or tool could have run.
    Started,
}

/// Durable outcome classification for a continuation effect boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationEffectOutcome {
    /// Host loss occurred after the effect fence; the approved effect may have happened.
    Indeterminate,
}

/// Typed host-visible projection for a continuation whose effect outcome cannot be proven.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContinuationEffectState {
    /// Last durable effect boundary crossed by the continuation.
    pub phase: ContinuationEffectPhase,
    /// Outcome classification exposed for recovery decisions.
    pub outcome: ContinuationEffectOutcome,
}

impl ContinuationEffectState {
    /// Build the fail-closed projection for a started continuation interrupted by host loss.
    #[must_use]
    pub const fn indeterminate() -> Self {
        Self {
            phase: ContinuationEffectPhase::Started,
            outcome: ContinuationEffectOutcome::Indeterminate,
        }
    }

    /// Insert this projection into durable run metadata.
    ///
    /// # Errors
    ///
    /// Returns a serialization error when the projection cannot be represented as JSON.
    pub fn insert_into(&self, metadata: &mut Map<String, Value>) -> Result<(), serde_json::Error> {
        metadata.insert(
            CONTINUATION_EFFECT_METADATA_KEY.to_string(),
            serde_json::to_value(self)?,
        );
        Ok(())
    }

    /// Decode a typed effect projection from durable run metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when present effect metadata is malformed.
    pub fn from_metadata(metadata: &Map<String, Value>) -> Result<Option<Self>, serde_json::Error> {
        metadata
            .get(CONTINUATION_EFFECT_METADATA_KEY)
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
    }
}

/// Exclusive durable claim acquired before a waiting run may execute a continuation.
///
/// Claims deliberately have no automatic expiry. A claimant may release a claim only before
/// continuation admission. Admission advances it to `Admitted`; the store then atomically checks
/// the live admission fence and advances it to `Started` immediately before any effect. After that
/// transition the claim is consumed only with terminal source-run evidence. This fails closed
/// after process loss instead of risking duplicate external tool effects.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HitlResumeClaim {
    /// Caller-generated unique ownership token.
    pub claim_id: String,
    /// Session containing the waiting run.
    pub session_id: SessionId,
    /// Waiting source run.
    pub run_id: RunId,
    /// Current claim phase.
    pub state: HitlResumeClaimState,
    /// Claim creation time.
    pub created_at: DateTime<Utc>,
}

impl starweaver_core::VersionedRecord for HitlResumeClaim {
    const SCHEMA: &'static str = "starweaver.session.hitl_resume_claim";
}

impl HitlResumeClaim {
    /// Build a claim for one waiting run.
    #[must_use]
    pub const fn new(
        claim_id: String,
        session_id: SessionId,
        run_id: RunId,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            claim_id,
            session_id,
            run_id,
            state: HitlResumeClaimState::Preflight,
            created_at,
        }
    }

    /// Return whether this is a valid newly acquired preflight claim.
    #[must_use]
    pub fn is_valid_preflight(&self) -> bool {
        self.state == HitlResumeClaimState::Preflight && !self.claim_id.trim().is_empty()
    }
}
