//! Whole-machine sharing activates only after the proposed exposure is clean.

use super::{
    cursor::{LocalSequence, Revision},
    identity::{AccountId, DeviceId, Digest, HubId, OperationId},
    proof::SignedStatement,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposureIntent {
    pub hub: HubId,
    pub operation: OperationId,
    pub machine: DeviceId,
    pub generation: Revision,
    pub holders: Vec<AccountId>,
    pub policy_revision: Revision,
    pub plan_digest: Digest,
}

/// Persisted only after publication is fenced and all ineligible managed
/// copies are removed, including staging and readable database/WAL residue.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExposureReceipt {
    pub hub: HubId,
    pub operation: OperationId,
    pub machine: DeviceId,
    pub generation: Revision,
    pub plan_digest: Digest,
    pub completed_local: LocalSequence,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedExposureReceipt {
    pub proof: SignedStatement,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ShareActivation {
    Pending { intent: ExposureIntent, reason: Option<String> },
    Active { generation: Revision },
    LegacyActiveWithoutCleanup,
    Cancelled { operation: OperationId },
}
