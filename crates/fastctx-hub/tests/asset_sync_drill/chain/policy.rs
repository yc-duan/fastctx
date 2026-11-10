use super::*;
use fastctx_sync::{
    identity::{AccountId, DeviceId, ObjectKey},
    registry::RegisteredKind,
    store::{access::*, source_authorize::*},
};

pub(super) struct Policy(pub AccountId, pub DeviceId);
impl Policy {
    fn decide(&self, c: &AccessContext) -> Decision {
        if matches!(c.principal, Principal::LocalService)
            || matches!(&c.principal, Principal::Account { account, .. } if account == &self.0)
        {
            Decision::Allowed { revision: c.observed_acl }
        } else {
            Decision::Denied { revision: c.observed_acl, reason: "outside this isolated account".into() }
        }
    }
}
impl AccessPolicy for Policy {
    fn can_read(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_read_object(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey) -> Decision {
        self.decide(c)
    }
    fn can_write(&self, c: &AccessContext, _: RegisteredKind) -> Decision {
        self.decide(c)
    }
    fn can_keep(&self, c: &AccessContext, _: RegisteredKind, _: &ObjectKey, use_: &HoldingUse) -> Decision {
        if let HoldingUse::RuntimeSource { host } = use_
            && (host.account != self.0 || host.host != self.1)
        {
            return Decision::Unavailable;
        }
        self.decide(c)
    }
    fn can_execute(&self, c: &AccessContext, _: &DeviceId) -> Decision {
        Decision::Denied { revision: c.observed_acl, reason: "the fixture executes no host".into() }
    }
}
pub(super) struct Gate(pub ObjectKey);
impl Gate {
    fn check(&self, payloads: &[SourcePayload<'_>], revision: fastctx_sync::cursor::Revision) -> Decision {
        for payload in payloads {
            let result = fastctx_sync::conversation::source::event::decode(payload.bytes).and_then(|event| {
                fastctx_sync::conversation::source::event::validate_record(
                    &self.0.object,
                    &self.0.scope,
                    &event,
                    payload.record,
                    payload.bytes,
                )
            });
            if let Err(reason) = result {
                return Decision::Denied { revision, reason };
            }
        }
        Decision::Allowed { revision }
    }
}
impl SourceAuthorizer for Gate {
    fn kind(&self) -> &'static str {
        KIND
    }
    fn authorize(&self, change: PreparedSourceChange<'_>) -> Decision {
        self.check(change.payloads, change.target.custody.context.observed_acl)
    }
    fn authorize_received(&self, change: ReceivedSourceBatch<'_>) -> Decision {
        self.check(change.payloads, change.custody.context.observed_acl)
    }
}
