//! Synchronization reuses the existing account and record policy.
//!
//! Services supply authenticated, committed identity/provenance snapshots. No
//! request deserializes into these assembly inputs, and policy checks perform no IO.

mod policy;
mod readers;
mod source;

use crate::{
    directory::{Directory, Member},
    directory_index::{Index, Indexed},
    sign::PublicKey,
};
use fastctx_sync::{
    cursor::Holder,
    identity::{AccountId, DeviceId, HubId, ObjectKey, ScopeKey},
    scope::{AccountIdentity, VerifiedScope},
    store::{Error, access::HoldingUse},
};
use std::{collections::BTreeMap, sync::Arc};

/// Immutable, indexed membership from the authenticated View plus the persisted
/// account-incarnation mapping. Account names are not rehashed after a rename.
pub struct DirectorySnapshot {
    hub: HubId,
    directory: Directory,
    index: Index,
    accounts: BTreeMap<AccountId, AccountIdentity>,
    names: BTreeMap<fastctx_types::AccountName, AccountId>,
    grants: BTreeMap<(DeviceId, AccountId), fastctx_sync::cursor::Revision>,
}
impl DirectorySnapshot {
    pub fn from_authenticated(hub: HubId, directory: Directory, accounts: Vec<AccountIdentity>) -> Result<Self, Error> {
        let index = Index::new(&directory);
        let indexed = Indexed::new(&directory, &index);
        let mut mappings = BTreeMap::new();
        let mut names = BTreeMap::new();
        for account in accounts {
            if indexed.account(&account.name).is_none_or(|current| current.opened != account.opened)
                || names.insert(account.name.clone(), account.id.clone()).is_some()
                || mappings.insert(account.id.clone(), account).is_some()
            {
                return Err(Error::Denied("account mapping differs from authenticated membership".into()));
            }
        }
        Ok(Self { hub, directory, index, accounts: mappings, names, grants: BTreeMap::new() })
    }
    /// Service composition loads these from its durable grant ledger. Current
    /// Share rows and wall-clock times cannot reconstruct historical grants.
    pub fn with_grants(mut self, grants: Vec<MachineGrant>) -> Result<Self, Error> {
        let mut current = BTreeMap::new();
        for grant in grants {
            let Some(account) = self.identity(&grant.account) else {
                return Err(Error::Denied("grant names an unknown account incarnation".into()));
            };
            let Some(machine) = self.member(&grant.machine) else {
                return Err(Error::Denied("grant names an unknown machine".into()));
            };
            if grant.revision.0 == 0
                || self.indexed().authorize(&account.name, crate::authorization::Action::Hold(&machine.name)).is_err()
                || current.insert((grant.machine, grant.account), grant.revision).is_some()
            {
                return Err(Error::Denied("grant snapshot differs from current membership".into()));
            }
        }
        self.grants = current;
        Ok(self)
    }
    fn indexed(&self) -> Indexed<'_> {
        Indexed::new(&self.directory, &self.index)
    }
    fn member(&self, id: &DeviceId) -> Option<&Member> {
        let bytes = std::array::from_fn(|i| {
            u8::from_str_radix(&id.as_str()[2 + i * 2..4 + i * 2], 16).expect("validated device identity")
        });
        self.indexed().by_key(&PublicKey::from_bytes(bytes))
    }
    fn identity(&self, id: &AccountId) -> Option<&AccountIdentity> {
        self.accounts.get(id).filter(|mapping| {
            self.indexed().account(&mapping.name).is_some_and(|current| current.opened == mapping.opened)
        })
    }
    fn account(&self, id: &AccountId) -> Option<&AccountIdentity> {
        self.identity(id).filter(|mapping| self.indexed().account(&mapping.name).is_some_and(|current| !current.locked))
    }
}

/// An active grant incarnation, supplied only by the authenticated ledger owner.
#[derive(Clone)]
pub struct MachineGrant {
    pub machine: DeviceId,
    pub account: AccountId,
    pub revision: fastctx_sync::cursor::Revision,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourcePhase {
    Active,
    /// Retained only until the hub durably acknowledges the sealed source tail
    /// and all content. This phase does not restore execution or interactive IO.
    AwaitingHub,
    Released,
}

/// Supplied from the trusted source registry, never from the transfer request.
/// `AwaitingHub` is persistent and ends only after complete hub content ACKs.
/// Job provenance includes its old reader routing, verified from dispatch/job
/// and PageSource/consent where applicable. Project membership alone does not
/// authorize routing a private page job's output into the whole project scope.
#[derive(Clone)]
pub struct SourceCustody {
    pub object: ObjectKey,
    pub kind: String,
    pub binding: HoldingUse,
    pub phase: SourcePhase,
}

#[derive(Clone)]
pub struct Snapshot {
    pub directory: Arc<DirectorySnapshot>,
    pub scope: Arc<VerifiedScope>,
    pub holder: Holder,
    pub enabled: bool,
    pub source: Option<SourceCustody>,
}

/// The service's committed read model. Implementations return the current scope
/// and requested object's exact provenance from memory, without other IO. The
/// writer additionally fences ACL/publication generations in its own transaction.
pub trait AccessState: Send + Sync {
    fn current(&self, scope: &ScopeKey, object: Option<&ObjectKey>) -> Option<Snapshot>;
}

pub struct DirectoryPolicy {
    state: Arc<dyn AccessState>,
}
impl DirectoryPolicy {
    /// Service composition only; ordinary features receive typed asset handles.
    pub fn from_authenticated(state: Arc<dyn AccessState>) -> Self {
        Self { state }
    }
}
