//! Durable signatures retain the exact bytes signed by the original producer.
//! This module encodes evidence; verification remains in the network adapter.

pub use super::acceptance::{
    AcceptanceBinding, AcceptanceFragment, AcceptedActor, CommittedFragment, SignedAcceptanceFragment,
};
use super::{
    cursor::{Revision, Sequence, StreamCursor},
    identity::{DeviceId, HubId, OperationId, ProducerEpoch, ScopeKey, StreamId},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bytes(pub Vec<u8>);
impl Serialize for Bytes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&URL_SAFE_NO_PAD.encode(&self.0))
    }
}
impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        URL_SAFE_NO_PAD.decode(String::deserialize(d)?).map(Self).map_err(serde::de::Error::custom)
    }
}

macro_rules! fixed_bytes {
    ($name:ident, $length:literal) => {
        #[derive(Clone, PartialEq, Eq)]
        pub struct $name(pub [u8; $length]);
        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(concat!(stringify!($name), "(<redacted>)"))
            }
        }
        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(&URL_SAFE_NO_PAD.encode(self.0))
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let bytes = Bytes::deserialize(d)?.0;
                let array = bytes
                    .try_into()
                    .map_err(|_| serde::de::Error::custom(concat!("invalid ", stringify!($name), " length")))?;
                Ok(Self(array))
            }
        }
    };
}
fixed_bytes!(SigningKey, 32);
fixed_bytes!(Signature, 64);

/// Each value maps to one registered net::sign domain, never an arbitrary name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProofPurpose {
    SourceBatchV1,
    AcceptedOperationV1,
    ProducerReleaseV1,
    EpochHandoverV1,
    ExposureReceiptV1,
    HubKeyRotationV1,
}

impl ProofPurpose {
    /// The network adapter maps these names to closed net::sign domain variants.
    pub const fn domain(self) -> &'static str {
        match self {
            Self::AcceptedOperationV1 => "fastctx-sync/accepted-operation/v1",
            Self::SourceBatchV1 => "fastctx-sync/source-batch/v1",
            Self::ProducerReleaseV1 => "fastctx-sync/producer-release/v1",
            Self::EpochHandoverV1 => "fastctx-sync/epoch-handover/v1",
            Self::ExposureReceiptV1 => "fastctx-sync/exposure-receipt/v1",
            Self::HubKeyRotationV1 => "fastctx-sync/hub-key-rotation/v1",
        }
    }
}

/// A release binds the successor and final cursor; an execution lease alone
/// cannot certify that the previous producer stopped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerRelease {
    pub predecessor: StreamCursor,
    pub successor: DeviceId,
    pub epoch: ProducerEpoch,
    pub operation: OperationId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedStatement {
    pub purpose: ProofPurpose,
    pub signer: SigningKey,
    pub data: Bytes,
    pub signature: Signature,
}

/// `data` encodes SourceBatch. Forwarders preserve all four proof fields, not a
/// decoded and reserialized batch. Its purpose must be SourceBatchV1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedSourceBatch {
    pub proof: SignedStatement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "basis", rename_all = "snake_case")]
pub enum HandoverBasis {
    Initial { registration: OperationId },
    Released { operation: OperationId, producer_release: SignedStatement },
    Restored { operation: OperationId, recovery_record: super::identity::Digest },
}

/// The hub confirms this binding under its pinned service key. A predecessor's
/// cutoff is permanent; records above it cannot arrive as legitimate late data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochDescriptor {
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub kind: String,
    pub schema: u16,
    pub epoch: ProducerEpoch,
    pub producer: DeviceId,
    pub predecessor: Option<StreamCursor>,
    pub closed_through: Option<Sequence>,
    pub operation: OperationId,
    pub control_sequence: Sequence,
    pub basis: HandoverBasis,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfirmedEpoch {
    pub proof: SignedStatement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubKeyRotation {
    pub hub: HubId,
    pub previous: SigningKey,
    pub next: SigningKey,
    pub revision: Revision,
    pub previous_control_through: Sequence,
    pub operation: OperationId,
}

/// Advertised over the already pinned TLS connection. A later key is accepted
/// only with the old key's rotation proof; old verification keys are retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubSigningIdentity {
    pub hub: HubId,
    pub key: SigningKey,
    pub revision: Revision,
    pub rotations: Vec<SignedStatement>,
}
