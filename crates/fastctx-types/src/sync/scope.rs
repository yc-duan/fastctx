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
