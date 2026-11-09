//! Display progress is explicit evidence of a rendered source prefix. Directory
//! caches and notification receipts cannot be converted into a read position.
use crate::{Error, context::AccountAccess, source::denied};
use fastctx_sync::{
    cursor::StreamCursor,
    identity::{AccountId, ObjectKey},
    store::access::{AccessContext, Principal},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Position {
    pub account: AccountId,
    pub position: StreamCursor,
}
impl Position {
    pub fn same_stream(&self, other: &Self) -> bool {
        self.account == other.account && same_stream(&self.position, &other.position)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Merge {
    Unchanged,
    Advanced(Position),
    DifferentStream,
}
pub fn merge(current: &Position, incoming: &Position) -> Merge {
    if !current.same_stream(incoming) {
        return Merge::DifferentStream;
    }
    if incoming.position.committed > current.position.committed {
        Merge::Advanced(incoming.clone())
    } else {
        Merge::Unchanged
    }
}
fn same_stream(a: &StreamCursor, b: &StreamCursor) -> bool {
    a.scope == b.scope && a.stream == b.stream && a.epoch == b.epoch
}

/// `displayed` comes from a verified display fence for this real user's request,
/// not from the requested position or the largest source number ever received.
pub fn authorize_observation(
    value: &Position,
    context: &AccessContext,
    object: &ObjectKey,
    access: &AccountAccess,
    displayed: &StreamCursor,
) -> Result<(), Error> {
    if !matches!(&context.principal, Principal::Account { account, .. } if account == &value.account)
        || !access.permits(context, object)
        || !same_stream(&value.position, displayed)
        || value.position.committed > displayed.committed
        || value.position.scope.hub != object.scope.hub
    {
        return Err(denied("read position lacks this user's verified display fence"));
    }
    Ok(())
}
