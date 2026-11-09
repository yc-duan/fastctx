//! What may happen in the scopes of this World's alone hub. Nobody else exists
//! there: the World is the only principal, and a user's reads reach the engines
//! as the World once the node's own admission let them through.
use fastctx_sync::{
    identity::{AccountId, DeviceId, HubId, ObjectKey},
    registry::RegisteredKind,
    store::access::{AccessContext, AccessPolicy, Decision, HoldingUse, Principal},
};

pub(super) struct AlonePolicy {
    alone: HubId,
    device: DeviceId,
    owner: AccountId,
}
impl AlonePolicy {
    pub(super) fn new(alone: HubId, device: DeviceId, owner: AccountId) -> Self {
        Self { alone, device, owner }
    }
    fn decide(&self, context: &AccessContext) -> Decision {
        let denied = |reason: &str| Decision::Denied { revision: context.observed_acl, reason: reason.into() };
        if context.scope.hub != self.alone {
            return denied("the scope is not under this World's alone hub");
        }
        match context.principal {
            Principal::LocalService => Decision::Allowed { revision: context.observed_acl },
            Principal::Account { .. } | Principal::Agent { .. } => denied("only this World acts under its alone hub"),
        }
    }
}
impl AccessPolicy for AlonePolicy {
    fn can_read(&self, context: &AccessContext, _kind: RegisteredKind) -> Decision {
        self.decide(context)
    }
    fn can_read_object(&self, context: &AccessContext, _kind: RegisteredKind, _object: &ObjectKey) -> Decision {
        self.decide(context)
    }
    fn can_write(&self, context: &AccessContext, _kind: RegisteredKind) -> Decision {
        self.decide(context)
    }
    /// Replicas, and the runtime sources this device hosts for the hub's owner.
    fn can_keep(
        &self,
        context: &AccessContext,
        _kind: RegisteredKind,
        _object: &ObjectKey,
        purpose: &HoldingUse,
    ) -> Decision {
        match purpose {
            HoldingUse::Replica => self.decide(context),
            HoldingUse::RuntimeSource { host }
                if host.host == self.device
                    && host.account == self.owner
                    && host.conversation.scope == context.scope =>
            {
                self.decide(context)
            }
            HoldingUse::RuntimeSource { .. } | HoldingUse::JobSource { .. } | HoldingUse::WorkspaceSource { .. } => {
                Decision::Denied {
                    revision: context.observed_acl,
                    reason: "a World on no hub keeps only what it hosts itself".into(),
                }
            }
        }
    }
    fn can_execute(&self, context: &AccessContext, _target: &DeviceId) -> Decision {
        Decision::Denied { revision: context.observed_acl, reason: "a World on no hub runs nothing for others".into() }
    }
}
