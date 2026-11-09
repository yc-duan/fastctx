//! Serializable work coordinates are not adoption or body-read capabilities.
use super::layout::{Address, Child};
use crate::{
    cursor::{HoldingPeriod, Revision},
    identity::{AccountId, DeviceId, Digest, ObjectKey, OperationId, ScopeKey, SourceId, StreamId},
    manifest::ContentRef,
    registry::REGISTRY,
    store::{
        access::{AccessContext, Custody, HoldingUse, Principal},
        roots::{RootUse, SupplyBasis},
        sql,
    },
};
use fastctx_storage::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SupplyOwner {
    Mutation,
    Source {
        source: SourceId,
    },
    Use {
        usage: RootUse,
    },
    /// Records an authority writes into this stream in the same operation, such
    /// as a hub's own directory records.
    Stream {
        stream: StreamId,
    },
}

pub(crate) struct SupplyRequest {
    pub operation: OperationId,
    pub custody: Custody,
    pub root: ContentRef,
    pub bases: Vec<SupplyBasis>,
    pub owner: SupplyOwner,
}

/// Persisted coordinates are replayable, but adoption rechecks the owning row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SupplyTicket {
    pub(in crate::blocks) scope: ScopeKey,
    pub(in crate::blocks) id: Digest,
    pub(in crate::blocks) generation: Revision,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum SupplyProgress {
    Pending { missing: Option<ContentRef> },
    Ready,
    Paused { reason: PauseReason },
    Rejected { node: ContentRef, reason: SupplyFailure },
    Cancelled,
    Consumed,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum PauseReason {
    HoldingChanged,
    SupplyChanged,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum SupplyFailure {
    MissingSupply,
    BasisReleased,
    WrongPurpose,
    InvalidPosition,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub(crate) enum CancelReason {
    Abandoned,
    UploadCancelled,
    Revoked,
    Deleted,
}

#[derive(Clone, Copy)]
pub(crate) struct SupplyBudget {
    pub(super) nodes: usize,
    pub(super) page_bytes: usize,
}
impl SupplyBudget {
    pub(crate) fn new(nodes: usize, page_bytes: usize) -> Result<Self> {
        if nodes == 0 || nodes > 64 || page_bytes == 0 || page_bytes > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(sql::invalid("supply traversal exceeds its node or page budget"));
        }
        Ok(Self { nodes, page_bytes })
    }
}
impl Default for SupplyBudget {
    fn default() -> Self {
        Self::new(64, fastctx_types::sync::PAGE_BYTES as usize).expect("fixed supply traversal bounds")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) enum SavedPrincipal {
    Account { account: AccountId, device: DeviceId },
    Agent { account: AccountId, device: DeviceId, conversation: ObjectKey },
    LocalService,
}
impl SavedPrincipal {
    fn capture(principal: &Principal) -> Self {
        match principal {
            Principal::Account { account, device } => {
                Self::Account { account: account.clone(), device: device.clone() }
            }
            Principal::Agent { account, device, conversation } => {
                Self::Agent { account: account.clone(), device: device.clone(), conversation: conversation.clone() }
            }
            Principal::LocalService => Self::LocalService,
        }
    }
    fn restore(&self) -> Principal {
        match self {
            Self::Account { account, device } => {
                Principal::Account { account: account.clone(), device: device.clone() }
            }
            Self::Agent { account, device, conversation } => Principal::Agent {
                account: account.clone(),
                device: device.clone(),
                conversation: conversation.clone(),
            },
            Self::LocalService => Principal::LocalService,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Request {
    pub operation: OperationId,
    pub object: ObjectKey,
    pub kind: String,
    pub holding: HoldingPeriod,
    pub purpose: HoldingUse,
    pub principal: SavedPrincipal,
    pub root: ContentRef,
    pub bases: Vec<SupplyBasis>,
    pub owner: SupplyOwner,
}
impl Request {
    pub fn capture(request: &SupplyRequest) -> Result<Self> {
        if request.bases.len() > 8 || matches!(request.owner, SupplyOwner::Use { usage: RootUse::Timeline { .. } }) {
            return Err(sql::invalid("supply reuse bases or usage exceed this entry point"));
        }
        request.root.validate().map_err(sql::invalid)?;
        let custody = &request.custody;
        if request.root.id().domain != custody.object.scope {
            return Err(sql::invalid("supply root differs from its object scope"));
        }
        let mut bases = std::collections::BTreeMap::new();
        for basis in &request.bases {
            bases.insert(encode(basis)?, basis.clone());
        }
        let value = Self {
            operation: request.operation.clone(),
            object: custody.object.clone(),
            kind: custody.kind.registration().id.into(),
            holding: custody.holding.clone(),
            purpose: custody.purpose.clone(),
            principal: SavedPrincipal::capture(&custody.context.principal),
            root: request.root.clone(),
            bases: bases.into_values().collect(),
            owner: request.owner.clone(),
        };
        if encode(&value)?.len() > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(sql::invalid("supply request exceeds its metadata budget"));
        }
        Ok(value)
    }
    pub fn custody(&self) -> Result<Custody> {
        Ok(Custody {
            context: AccessContext {
                principal: self.principal.restore(),
                scope: self.object.scope.clone(),
                observed_acl: self.holding.acl_revision,
            },
            kind: match REGISTRY.entries().iter().find(|kind| kind.id == self.kind).map(|kind| kind.class) {
                Some(crate::registry::DataClass::Asset) => {
                    REGISTRY.asset(&self.kind).map_err(sql::invalid)?.registered()
                }
                Some(crate::registry::DataClass::Derived) => {
                    REGISTRY.projection(&self.kind).map_err(sql::invalid)?.registered()
                }
                _ => return Err(sql::invalid("supply custody needs a registered content kind")),
            },
            object: self.object.clone(),
            holding: self.holding.clone(),
            purpose: self.purpose.clone(),
        })
    }
    pub fn identity(&self) -> Result<Digest> {
        let mut bytes = b"fastctx-sync/local-supply/v1\0".to_vec();
        bytes.extend(encode(self)?);
        Ok(Digest::of(&bytes))
    }
}

/// Basis descent is persisted separately from candidate expansion so large
/// page pairs do not need simultaneous decoding or a copied closure index.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Work {
    pub node: Child,
    pub bases: Vec<Child>,
    pub cursor: usize,
    pub seek: Option<Child>,
    pub expand: bool,
    pub child: usize,
    pub depth: usize,
}
impl Work {
    pub fn root(request: &Request) -> Self {
        let bases = request
            .bases
            .iter()
            .map(|basis| Child {
                root: match basis {
                    SupplyBasis::Published { root } => root.clone(),
                    SupplyBasis::Held { lease } => lease.root.clone(),
                },
                address: Address::default(),
            })
            .collect();
        Self {
            node: Child { root: request.root.clone(), address: Address::default() },
            bases,
            cursor: 0,
            seek: None,
            expand: false,
            child: 0,
            depth: 0,
        }
    }
}

pub(super) fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
pub(super) fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|error| sql::invalid(error.to_string()))
}
