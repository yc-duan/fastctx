//! One policy distinguishes ordinary replicas from authorized source custody.

use crate::{
    cursor::{HoldingPeriod, Revision},
    identity::{AccountId, DeviceId, ObjectKey, OperationId, ProducerEpoch, ScopeKey, WorkspaceEpoch},
    registry::RegisteredKind,
};
#[derive(Clone, Debug)]
pub enum Principal {
    /// A real user request, whose device login is checked by the adapter.
    Account {
        account: AccountId,
        device: DeviceId,
    },
    /// Delegated execution remains an agent even on a device logged into the account.
    Agent {
        account: AccountId,
        device: DeviceId,
        conversation: ObjectKey,
    },
    LocalService,
}
#[derive(Clone, Debug)]
pub struct AccessContext {
    pub principal: Principal,
    pub scope: ScopeKey,
    pub observed_acl: Revision,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostBinding {
    pub conversation: ObjectKey,
    pub account: AccountId,
    pub host: DeviceId,
    pub epoch: ProducerEpoch,
}
/// Page jobs retain their real project/page owner rather than inventing a
/// conversation. Coordinates are checked against authenticated dispatch, the
/// durable PageSource/job and its consent before source provenance is registered.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobSourceOwner {
    Resident { conversation: ObjectKey },
    Page { project: ObjectKey, page: ObjectKey },
}

/// Verified against authenticated dispatch and the durable job record. Finishing
/// the job does not end source custody; revocation and deletion still apply.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JobBinding {
    pub job: ObjectKey,
    pub epoch: ProducerEpoch,
    pub owner: JobSourceOwner,
    pub account: AccountId,
    pub executor: DeviceId,
    pub dispatcher: DeviceId,
    pub dispatch: OperationId,
}

/// One artifact workspace per conversation and machine, shared by its jobs.
/// The epoch is registered before ingestion; discovering a directory is not
/// evidence of ownership. A stream's producer epoch may change independently.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceBinding {
    pub conversation: ObjectKey,
    pub account: AccountId,
    pub machine: DeviceId,
    pub epoch: WorkspaceEpoch,
    pub registration: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// Final object/body gate, including authenticated derived-source inheritance.
    fn can_read_object(&self, context: &AccessContext, kind: RegisteredKind, object: &ObjectKey) -> Decision;
    fn can_write(&self, context: &AccessContext, kind: RegisteredKind) -> Decision;
    /// Evaluates custody of the service's actual holder. Period reconciliation
    /// calls this as LocalService with persisted object and purpose bindings;
    /// retaining content grants no read, write or execution permission.
    fn can_keep(
        &self,
        context: &AccessContext,
        kind: RegisteredKind,
        object: &ObjectKey,
        purpose: &HoldingUse,
    ) -> Decision;
    fn can_execute(&self, context: &AccessContext, target: &DeviceId) -> Decision;
}
