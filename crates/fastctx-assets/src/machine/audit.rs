//! Management facts are hub-authored. Reader routing uses the original frozen
//! sets, not an action's privacy label or today's machine ownership.
use crate::{
    Error, json,
    source::{Binding, denied},
};
use fastctx_sync::{
    identity::{AccountId, DeviceId, OperationId, ScopeKey},
    store::ingest::SourceTarget,
};
use fastctx_types::{
    machine_log::Record,
    sync::scope::{ScopeDescriptor, ScopeSubject, ScopeVisibility},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Audit {
    /// Every audience copy retains this same logical event identity.
    pub operation: OperationId,
    pub record: Record,
}
impl Audit {
    pub fn validate(&self) -> Result<(), Error> {
        if self.record.id == 0 {
            return Err(json::invalid("audit requires its original hub record identity"));
        }
        Ok(())
    }
}
/// Account IDs already include the original `opened` incarnation. Device readers
/// have not been reinterpreted as whichever account owns that machine today.
#[derive(Clone, Debug, Default)]
pub struct FrozenReaders {
    pub accounts: BTreeSet<AccountId>,
    pub devices: BTreeSet<DeviceId>,
    pub whole: bool,
}
/// Trusted components recovered from the management transaction's directory
/// snapshot (or proven old reader mappings). A holder is routed through Segment,
/// not silently converted to an irrevocable direct account recipient.
pub enum Component<'a> {
    Account { account: &'a AccountId, scope: &'a ScopeDescriptor },
    Segment { holders: &'a BTreeSet<AccountId>, scope: &'a ScopeDescriptor },
    DeviceAccount { device: &'a DeviceId, account: &'a AccountId, scope: &'a ScopeDescriptor },
    Administrators { scope: &'a ScopeDescriptor },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fanout {
    event: Audit,
    scopes: BTreeSet<ScopeKey>,
    /// Device-only readers with no proven account mapping remain on that device.
    local_devices: BTreeSet<DeviceId>,
    unresolved_accounts: BTreeSet<AccountId>,
    missing_administrators: bool,
}
impl Fanout {
    pub fn scopes(&self) -> &BTreeSet<ScopeKey> {
        &self.scopes
    }
    pub fn local_devices(&self) -> &BTreeSet<DeviceId> {
        &self.local_devices
    }
    pub fn unresolved_accounts(&self) -> &BTreeSet<AccountId> {
        &self.unresolved_accounts
    }
    pub fn missing_administrators(&self) -> bool {
        self.missing_administrators
    }
    pub fn complete(&self) -> bool {
        self.unresolved_accounts.is_empty() && !self.missing_administrators
    }
}

/// Produces the whole plan or an error, never a truncated audience. Callers budget
/// the input snapshot; the plan has at most one scope per supplied component.
/// Revocation of a machine grant removes access to every one of its segments.
pub fn route(
    event: &Audit,
    readers: &FrozenReaders,
    hub: &fastctx_sync::identity::HubId,
    components: &[Component<'_>],
) -> Result<Fanout, Error> {
    event.validate()?;
    if super::members::applies(&event.record.action) {
        return Err(Error::Unavailable("all-member audit requires its Members grant segment".into()));
    }
    let mut plan = Fanout {
        event: event.clone(),
        scopes: BTreeSet::new(),
        local_devices: readers.devices.clone(),
        unresolved_accounts: readers.accounts.clone(),
        missing_administrators: readers.whole,
    };
    for component in components {
        let scope = match component {
            Component::Account { account, scope } => {
                if !readers.accounts.contains(*account) || !account_scope(scope, account) {
                    return Err(denied("audit account component exceeds the original readers"));
                }
                plan.unresolved_accounts.remove(*account);
                *scope
            }
            Component::Segment { holders, scope } => {
                if holders.is_empty()
                    || !holders.is_subset(&readers.accounts)
                    || !matches!(scope.subject, ScopeSubject::Machine { .. })
                    || !matches!(scope.visibility, ScopeVisibility::SinceGrant { opened_acl } if opened_acl.0 > 0 && opened_acl <= scope.acl_revision)
                {
                    return Err(denied("audit machine component lacks its original grant segment"));
                }
                for holder in *holders {
                    plan.unresolved_accounts.remove(holder);
                }
                *scope
            }
            Component::DeviceAccount { device, account, scope } => {
                if !readers.devices.contains(*device) || !account_scope(scope, account) {
                    return Err(denied("audit device component lacks its proven account mapping"));
                }
                plan.local_devices.remove(*device);
                *scope
            }
            Component::Administrators { scope } => {
                if !readers.whole || scope.subject != ScopeSubject::Administrators {
                    return Err(denied("audit did not have a whole reader"));
                }
                plan.missing_administrators = false;
                *scope
            }
        };
        if scope.scope.hub != *hub {
            return Err(denied("audit fanout cannot cross hubs"));
        }
        crate::account::directory::decode(&json::bytes(scope)?)?;
        plan.scopes.insert(scope.scope.clone());
    }
    Ok(plan)
}
fn account_scope(scope: &ScopeDescriptor, account: &AccountId) -> bool {
    scope.subject == ScopeSubject::Account && scope.owner.account().is_some_and(|owner| owner.id == *account)
}

/// `committed` and `plan` come from the real management transaction: actor,
/// outcome and audience are not accepted from a node or reconstructed from labels.
/// `record_operation` is taken from the verified source record envelope.
pub fn authorize_source(
    value: &Audit,
    record_operation: &OperationId,
    target: &SourceTarget,
    binding: &Binding,
    committed: &Audit,
    plan: &Fanout,
) -> Result<(), Error> {
    value.validate()?;
    if super::members::applies(&value.record.action) {
        return Err(Error::Unavailable("all-member audit requires its Members grant segment".into()));
    }
    binding.check(target, "machine.audit")?;
    binding.hub()?;
    if &value.operation != record_operation
        || value != committed
        || value != &plan.event
        || !plan.complete()
        || !plan.scopes.contains(&binding.object.scope)
    {
        return Err(denied("audit differs from its committed management fact or complete fanout"));
    }
    Ok(())
}
