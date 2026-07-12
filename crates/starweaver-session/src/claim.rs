//! Durable ownership claims for HITL continuation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use starweaver_core::{RunId, SessionId};

/// Durable resume-claim phase.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HitlResumeClaimState {
    /// Ownership is acquired but no continuation hook or tool may have executed yet.
    Preflight,
    /// Continuation execution may have produced external effects; release is forbidden.
    Started,
}

/// Exclusive durable claim acquired before a waiting run may execute a continuation.
///
/// Claims deliberately have no automatic expiry. A claimant may release a claim only before
/// continuation execution starts; after execution starts, the claim is consumed atomically with
/// terminal source-run evidence. This fails closed after process loss instead of risking duplicate
/// external tool effects.
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
