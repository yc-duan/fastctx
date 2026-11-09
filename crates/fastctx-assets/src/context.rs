//! Trusted, current business facts supplied by service composition, never wire
//! claims. Providers do no IO while a foundation writer is authorizing a change.
use fastctx_sync::{
    cursor::Revision,
    identity::{AccountId, ObjectKey},
    store::access::{AccessContext, Principal},
};
use fastctx_types::MachineName;
use std::collections::BTreeSet;

/// The adapter must bind these facts to the requested principal and exact
/// account object using its authenticated directory and incarnation mapping.
/// `hands` is the old source-to-target Hands decision, not mere membership or
/// online status. Old-node delegation needs the real internal service port;
/// it must not be represented by fabricated Account or Agent facts here.
#[derive(Clone, Debug)]
pub struct AccountAccess {
    pub object: ObjectKey,
    pub owner: AccountId,
    pub acl_revision: Revision,
    pub logged_in: bool,
    pub act_for: bool,
    pub phone: bool,
    pub hands: BTreeSet<MachineName>,
}
impl AccountAccess {
    pub(crate) fn permits(&self, context: &AccessContext, object: &ObjectKey) -> bool {
        if &self.object != object || context.scope != object.scope || self.acl_revision != context.observed_acl {
            return false;
        }
        match &context.principal {
            Principal::Account { account, .. } => account == &self.owner && self.logged_in,
            Principal::Agent { account, conversation, .. } => {
                account == &self.owner && self.act_for && conversation.scope.hub == object.scope.hub
            }
            Principal::LocalService => false,
        }
    }
}

/// Supplies a committed in-memory snapshot. The foundation separately checks
/// current scope/holding fences and repeats authorization in its transaction.
pub trait AccountState: Send + Sync {
    fn current(&self, context: &AccessContext, object: &ObjectKey) -> Option<AccountAccess>;
}
