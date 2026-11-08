//! One policy distinguishes ordinary replicas from authorized source custody.

use crate::{
    cursor::{HoldingPeriod, Revision},
    identity::{AccountId, DeviceId, ObjectKey, OperationId, ProducerEpoch, ScopeKey, WorkspaceEpoch},
    registry::RegisteredKind,
};
#[derive(Clone, Debug)]
pub enum Principal {
    Account { account: AccountId, device: DeviceId },
    Agent { account: AccountId, device: DeviceId, conversation: ObjectKey },
    LocalService,
}
#[derive(Clone, Debug)]
pub struct AccessContext {
    pub principal: Principal,
    pub scope: ScopeKey,
    pub observed_acl: Revision,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostBinding {
    pub conversation: ObjectKey,
    pub account: AccountId,
    pub host: DeviceId,
    pub epoch: ProducerEpoch,
}
/// Verified against authenticated dispatch and the durable job record. Finishing
/// the job does not end source custody; revocation and deletion still apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobBinding {
    pub job: ObjectKey,
    pub epoch: ProducerEpoch,
    pub conversation: ObjectKey,
    pub account: AccountId,
    pub executor: DeviceId,
    pub dispatcher: DeviceId,
    pub dispatch: OperationId,
}

/// One artifact workspace per conversation and machine, shared by its jobs.
/// The epoch is registered before ingestion; discovering a directory is not
/// evidence of ownership. A stream's producer epoch may change independently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceBinding {
    pub conversation: ObjectKey,
    pub account: AccountId,
    pub machine: DeviceId,
    pub epoch: WorkspaceEpoch,
    pub registration: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HoldingUse {
    Replica,
    RuntimeSource { host: HostBinding },
    JobSource { job: JobBinding },
    WorkspaceSource { workspace: WorkspaceBinding },
}

/// The object and root are verified against committed metadata or a registered
/// staging intent; a remote caller cannot relabel a plan block as an image.
/// Source custody additionally requires the object's registered provenance to
/// match the job or workspace binding, not merely the same conversation scope.
#[derive(Clone)]
pub struct Custody {
    pub context: AccessContext,
    pub kind: RegisteredKind,
    pub object: ObjectKey,
    pub purpose: HoldingUse,
    pub holding: HoldingPeriod,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed { revision: Revision },
    Denied { revision: Revision, reason: String },
    Unavailable,
}
pub trait AccessPolicy: Send + Sync {
    fn can_read(&self, context: &AccessContext, kind: RegisteredKind) -> Decision;
    fn can_write(&self, context: &AccessContext, kind: RegisteredKind) -> Decision;
    fn can_keep(&self, context: &AccessContext, kind: RegisteredKind, purpose: &HoldingUse) -> Decision;
    fn can_execute(&self, context: &AccessContext, target: &DeviceId) -> Decision;
}
