//! What the hub's own engines may do. The hub's service keeps and reads
//! everything it describes; requests from devices reach the engines only
//! through the service, which judges them against the hub's directory.
use fastctx_sync::{
    identity::{DeviceId, ObjectKey},
    registry::RegisteredKind,
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};

pub(super) struct HubPolicy;
impl HubPolicy {
    fn decide(&self, context: &AccessContext) -> Decision {
        match context.principal {
            Principal::LocalService => Decision::Allowed { revision: context.observed_acl },
            Principal::Account { .. } | Principal::Agent { .. } => Decision::Denied {
                revision: context.observed_acl,
                reason: "devices reach the hub's engines only through its sync service".into(),
            },
        }
    }
}
impl AccessPolicy for HubPolicy {
    fn can_read(&self, context: &AccessContext, _kind: RegisteredKind) -> Decision {
        self.decide(context)
    }
    fn can_read_object(&self, context: &AccessContext, _kind: RegisteredKind, _object: &ObjectKey) -> Decision {
        self.decide(context)
    }
    fn can_write(&self, context: &AccessContext, _kind: RegisteredKind) -> Decision {
        self.decide(context)
    }
    fn can_keep(
        &self,
        context: &AccessContext,
        _kind: RegisteredKind,
        _object: &ObjectKey,
        _: &HoldingUse,
    ) -> Decision {
        self.decide(context)
    }
    fn can_execute(&self, context: &AccessContext, _target: &DeviceId) -> Decision {
        Decision::Denied { revision: context.observed_acl, reason: "the hub runs nothing for devices".into() }
    }
}
