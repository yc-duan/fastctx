//! Alerts fan out by the original told account incarnations. Delivery receipts
//! remain runtime data, and reading an alert still requires a real user login.
use crate::{
    Error,
    context::AccountAccess,
    source::{Binding, denied},
};
use fastctx_sync::{
    identity::{AccountId, ObjectKey, OperationId},
    store::{
        access::{AccessContext, Principal},
        ingest::SourceTarget,
    },
};
use fastctx_types::{
    sync::scope::{ScopeDescriptor, ScopeSubject},
    trouble::Trouble,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alert {
    pub operation: OperationId,
    pub recipient: AccountId,
    pub trouble: Trouble,
}
pub struct Evidence<'a> {
    pub binding: &'a Binding,
    pub operation: &'a OperationId,
    pub trouble: &'a Trouble,
    pub told: &'a BTreeSet<AccountId>,
}
pub fn authorize_source(
    value: &Alert,
    record_operation: &OperationId,
    target: &SourceTarget,
    evidence: Evidence<'_>,
    scope: &ScopeDescriptor,
) -> Result<(), Error> {
    evidence.binding.check(target, "account.notice")?;
    evidence.binding.hub()?;
    if &value.operation != record_operation
        || &value.operation != evidence.operation
        || &value.trouble != evidence.trouble
        || !evidence.told.contains(&value.recipient)
        || scope.scope != evidence.binding.object.scope
        || scope.acl_revision != evidence.binding.acl_revision
        || scope.subject != ScopeSubject::Account
        || scope.owner.account().map(|owner| &owner.id) != Some(&value.recipient)
    {
        return Err(denied("alert differs from its original told recipient or hub fact"));
    }
    Ok(())
}
pub fn can_read(value: &Alert, context: &AccessContext, object: &ObjectKey, access: &AccountAccess) -> bool {
    matches!(&context.principal, Principal::Account { account, .. } if account == &value.recipient)
        && access.permits(context, object)
}
