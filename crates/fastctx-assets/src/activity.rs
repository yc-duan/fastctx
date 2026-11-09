//! Activity retains the executor and original account column. A queried Hour
//! aggregated across accounts is not an import source for an account asset.
use crate::{
    Error, json,
    source::{Binding, denied},
};
use fastctx_sync::{
    identity::{AccountId, DeviceId},
    store::ingest::SourceTarget,
};
use fastctx_types::{
    activity_book::{Deed, Hour},
    sync::scope::{ScopeDescriptor, ScopeSubject},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activity {
    pub executor: DeviceId,
    pub account: AccountId,
    /// Preserved from the row, never synthesized from today's logged-in account.
    pub original_account: String,
    /// The original deed key or the complete original hourly primary key.
    pub row: String,
    pub detail: Detail,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case", deny_unknown_fields)]
pub enum Detail {
    Deed(Deed),
    Hour(Hour),
}
impl Activity {
    pub fn validate(&self) -> Result<(), Error> {
        if self.original_account.is_empty() || self.row.is_empty() {
            return Err(json::invalid(
                "activity needs its original account and row identity; unresolved rows stay local",
            ));
        }
        Ok(())
    }
}
/// Comes from the executor journal or the exact legacy row plus its proven
/// account-incarnation mapping. An empty original account yields no evidence.
pub struct Evidence<'a> {
    pub binding: &'a Binding,
    pub executor: &'a DeviceId,
    pub account: &'a AccountId,
    pub original_account: &'a str,
    pub row: &'a str,
    pub detail: &'a Detail,
}
pub fn authorize_source(
    value: &Activity,
    target: &SourceTarget,
    evidence: Evidence<'_>,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    value.validate()?;
    evidence.binding.check(target, "account.activity")?;
    evidence.binding.device(evidence.executor)?;
    if &value.detail != evidence.detail
        || value.executor != *evidence.executor
        || value.account != *evidence.account
        || value.original_account != evidence.original_account
        || value.row != evidence.row
        || scope.scope != evidence.binding.object.scope
        || scope.acl_revision != evidence.binding.acl_revision
        || scope.subject != ScopeSubject::Account
        || scope.owner.account().map(|owner| &owner.id) != Some(evidence.account)
    {
        return Err(denied("activity differs from its executor row or proven account incarnation"));
    }
    Ok(())
}
