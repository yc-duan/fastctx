//! Policy adapters delegate to the existing network ownership rules.

use crate::{
    cursor::Revision,
    identity::{AccountId, DeviceId, ScopeKey},
};

/// A local service may maintain registered runtime state without an account
/// login. It cannot impersonate an account or authorize a remote operation.
#[derive(Clone, Debug)]
pub enum Principal {
    Account { account: AccountId, device: DeviceId },
    LocalService,
}

/// Identity from authentication or the local service composition root, never
/// inferred from a path and never deserialized from a remote request.
#[derive(Clone, Debug)]
pub struct AccessContext {
    pub principal: Principal,
    pub scope: ScopeKey,
    pub observed_acl: Revision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Allowed { revision: Revision },
    Denied { revision: Revision, reason: String },
    Unavailable,
}

/// Reading, retaining and executing are different grants. A stale decision is
/// not a capability: the backend checks the current policy at commit/send time.
pub trait AccessPolicy: Send + Sync {
    fn can_read(&self, context: &AccessContext) -> Decision;
    fn can_write(&self, context: &AccessContext) -> Decision;
    fn can_keep(&self, context: &AccessContext) -> Decision;
    fn can_execute(&self, context: &AccessContext, target: &DeviceId) -> Decision;
}
