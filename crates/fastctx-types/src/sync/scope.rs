//! Minimal ownership evidence, distinct from data replication and execution grants.

use super::{
    catalog::Lifecycle,
    cursor::{Revision, ScopeCursor, VersionRef},
    identity::{AccountId, DeviceId, Digest, ObjectId, ScopeKey, ViewId},
    proof::{Bytes, ConfirmedEpoch, SignedSourceBatch},
    transfer::KindVersion,
};
use crate::{AccountName, MachineName, access::TownId};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

/// A stable mapping established once by the hub. Renaming changes the name, not
/// the account identity; a new opening time identifies a different account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccountIdentity {
    pub id: AccountId,
    pub name: AccountName,
    pub opened: Timestamp,
}

/// Hub-owned administrator scopes outlive any one administrator account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "owner", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeOwner {
    Account { identity: AccountIdentity },
    Hub,
}
impl ScopeOwner {
    pub fn account(&self) -> Option<&AccountIdentity> {
        match self {
            Self::Account { identity } => Some(identity),
            Self::Hub => None,
        }
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Hub, Self::Hub) => true,
            (Self::Account { identity: a }, Self::Account { identity: b }) => a.id == b.id && a.opened == b.opened,
            _ => false,
        }
    }
}

/// A machine segment's opening fence is immutable. Regrant starts another
/// segment rather than exposing facts committed before the new grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "visibility", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeVisibility {
    Current,
    SinceGrant { opened_acl: Revision },
}

/// Runtime addresses, previous hosts, titles and record bodies are deliberately
/// absent. Projects use the grants already projected in the authenticated View.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "subject", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScopeSubject {
    Account,
    Conversation { conversation: ObjectId, shared_on: Option<MachineName>, born_seal: Option<String> },
    Project { project: ObjectId, town: TownId },
    Machine { device: DeviceId },
    Device { device: DeviceId },
    Administrators,
}

/// A single account.directory source record binds this ownership description.
/// Any permission-affecting update also advances acl_revision in that commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeDescriptor {
    pub format: u16,
    pub scope: ScopeKey,
    pub owner: ScopeOwner,
    pub subject: ScopeSubject,
    pub visibility: ScopeVisibility,
    pub revision: VersionRef,
    pub acl_revision: Revision,
    pub lifecycle: Lifecycle,
}

impl ScopeDescriptor {
    /// The rules every holder checks before trusting a description, whether it
    /// signs, verifies or installs it.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != 1
            || self.acl_revision.0 == 0
            || self.revision.revision.0 == 0
            || self.lifecycle == Lifecycle::Removed
        {
            return Err("invalid scope identity or revision".into());
        }
        if matches!(self.subject, ScopeSubject::Administrators) != matches!(self.owner, ScopeOwner::Hub) {
            return Err("only administrator scopes are hub-owned".into());
        }
        if let ScopeVisibility::SinceGrant { opened_acl } = self.visibility
            && (!matches!(self.subject, ScopeSubject::Machine { .. })
                || opened_acl.0 == 0
                || opened_acl > self.acl_revision)
        {
            return Err("grant visibility requires a valid immutable machine segment fence".into());
        }
        if let ScopeSubject::Conversation { shared_on, born_seal, .. } = &self.subject
            && (shared_on.is_some() != born_seal.is_some() || born_seal.as_ref().is_some_and(|seal| seal.len() > 4096))
        {
            return Err("shared conversation origin needs its bounded machine seal".into());
        }
        Ok(())
    }

    /// This does not order authority epochs; feed order is already established.
    /// Owner, subject and visibility never change, and every changed description
    /// advances its ACL generation.
    pub fn validate_successor(&self, previous: &Self) -> Result<(), String> {
        if self == previous {
            return Ok(());
        }
        if self.scope != previous.scope
            || !self.owner.same_identity(&previous.owner)
            || self.visibility != previous.visibility
            || self.revision == previous.revision
            || self.acl_revision <= previous.acl_revision
            || previous.lifecycle == Lifecycle::Deleted
            || !self.subject.same_subject(&previous.subject)
        {
            return Err("scope update changes identity or fails to advance its ACL fence".into());
        }
        Ok(())
    }
}

impl ScopeSubject {
    /// Whether two descriptions name the same subject; the shared-machine birth
    /// details of a conversation are properties, not its identity.
    pub fn same_subject(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Account, Self::Account) | (Self::Administrators, Self::Administrators) => true,
            (Self::Conversation { conversation: a, .. }, Self::Conversation { conversation: b, .. }) => a == b,
            (Self::Project { project: a, .. }, Self::Project { project: b, .. }) => a == b,
            (Self::Machine { device: a }, Self::Machine { device: b })
            | (Self::Device { device: a }, Self::Device { device: b }) => a == b,
            _ => false,
        }
    }
}

/// The scope an account's own facts and its directory live in.
pub fn account_scope(account: &AccountId) -> super::identity::ScopeId {
    subject_scope("account", account.as_str())
}
/// The scope of control facts about one device.
pub fn device_scope(device: &DeviceId) -> super::identity::ScopeId {
    subject_scope("device", device.as_str())
}
fn subject_scope(subject: &str, identity: &str) -> super::identity::ScopeId {
    let mut hash = Sha256::new();
    hash.update(b"fastctx.sync.subject-scope.v1\0");
    for field in [subject, identity] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field.as_bytes());
    }
    super::identity::ScopeId::from_bytes(hash.finalize().into())
}
/// The object whose source stream carries a directory scope's records.
pub fn directory_object(scope: &ScopeKey) -> super::identity::ObjectKey {
    let mut hash = Sha256::new();
    hash.update(b"fastctx.sync.directory-object.v1\0");
    hash.update(scope.hub.bytes());
    hash.update((scope.scope.as_str().len() as u64).to_be_bytes());
    hash.update(scope.scope.as_str().as_bytes());
    super::identity::ObjectKey { scope: scope.clone(), object: ObjectId::from_bytes(hash.finalize().into()) }
}

/// Exact payload bytes and the original one-record batch are retained together.
/// A scoped control cache does not confer custody of the account directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeProof {
    pub epoch: ConfirmedEpoch,
    pub batch: SignedSourceBatch,
    pub payload: Bytes,
}

/// A persisted, recipient-specific snapshot of the filtered scope directory.
/// The hub retains its directory feed boundaries with this snapshot identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryFence {
    pub recipient: DeviceId,
    pub view: ViewId,
    pub snapshot: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryCursor {
    pub fence: DiscoveryFence,
    pub after: ScopeKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeDiscoveryRead {
    pub cursor: Option<DiscoveryCursor>,
    /// Already durable directory cursors, supplied on reconnect in bounded pages.
    pub directories: Vec<ScopeCursor>,
    pub max_items: u32,
    pub max_bytes: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeOffer {
    pub binding: ScopeProof,
    pub cursor: ScopeCursor,
    pub kinds: Vec<KindVersion>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopesPage {
    pub fence: DiscoveryFence,
    pub entries: Vec<ScopeOffer>,
    /// Only directory heads represented by this bounded page. These become
    /// resumable after their complete fenced snapshot has been installed.
    pub directories: Vec<ScopeCursor>,
    pub next: Option<DiscoveryCursor>,
}
