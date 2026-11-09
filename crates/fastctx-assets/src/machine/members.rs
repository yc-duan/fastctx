//! Pure lazy segmentation for the hub's all-member audit events. This is a
//! routing decision, not a substitute for the foundation's Members scope type.
use crate::{Error, source::denied};
use fastctx_sync::identity::AccountId;
use fastctx_types::machine_log::Action;
use jiff::Timestamp;
use std::collections::{BTreeMap, BTreeSet};

pub fn applies(action: &Action) -> bool {
    matches!(
        action,
        Action::Transfer { .. } | Action::Settings { .. } | Action::Updates { .. } | Action::Dissolve { .. }
    )
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub opened_at: Timestamp,
    pub opened_accounts: BTreeSet<AccountId>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    Reuse,
    Open(Segment),
}

/// `members` is reconstructed at the event time, including original account
/// incarnations and lifecycles, never today's account directory. Frozen legacy
/// readers, when available, must agree exactly. Missing evidence is an error.
/// Deletions need no new segment; every segment's read gate checks live membership.
pub fn plan_legacy(
    at: Timestamp,
    members: &BTreeMap<AccountId, Timestamp>,
    frozen: Option<&BTreeSet<AccountId>>,
    current: Option<&Segment>,
) -> Result<Plan, Error> {
    let accounts: BTreeSet<_> = members.keys().cloned().collect();
    if members.values().any(|opened| *opened > at) || frozen.is_some_and(|frozen| frozen != &accounts) {
        return Err(denied("all-member audit lacks its original membership evidence"));
    }
    if let Some(current) = current {
        if current.opened_at > at {
            return Err(denied("membership segment replay is out of order"));
        }
        if accounts.is_subset(&current.opened_accounts) {
            return Ok(Plan::Reuse);
        }
    }
    Ok(Plan::Open(Segment { opened_at: at, opened_accounts: accounts }))
}
