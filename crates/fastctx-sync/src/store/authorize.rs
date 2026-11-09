//! Semantic authorization is separate from the scope-level write gate.

use super::{
    access::{AccessContext, Decision},
    mutation::{FieldEdit, ProposedObject},
};
use crate::catalog::ObjectHead;

/// Local preparation binds `before` to this object's accepted version or its
/// queued same-origin predecessor, then rechecks that binding inside the writer.
/// Authority acceptance supplies its own current body and the replayed intent;
/// a client's claimed before/candidate is never authority authorization evidence.
/// Handlers derive field deltas from these bounded, content-verified bytes.
pub struct PreparedChange<'a> {
    pub proposal: &'a ProposedObject,
    pub current: Option<&'a ObjectHead>,
    pub before: &'a [u8],
    pub after: &'a [u8],
    pub fields: &'a [FieldEdit],
}

/// Required for each writable kind. Runs inside the writer against the current
/// in-memory permission snapshot: no filesystem, network or second database IO.
/// Absence of this adapter disables writes; scope permission is not a substitute.
pub trait MutationAuthorizer: Send + Sync {
    fn kind(&self) -> &'static str;
    fn authorize(&self, context: &AccessContext, change: PreparedChange<'_>) -> Decision;
}
