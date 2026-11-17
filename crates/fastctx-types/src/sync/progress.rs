//! Local durable source progress does not imply a hub-confirmed producer epoch.
use super::{cursor::StreamCursor, identity::Digest};
use serde::{Deserialize, Serialize};

/// The same locally assigned item/operation/sequence survives epoch confirmation.
/// A local cursor has no fabricated epoch proof or prefix digest. Confirmation
/// builds the verifiable prefix from retained facts without rereading the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum StreamProgress {
    LocalPending { cursor: StreamCursor },
    Confirmed { cursor: StreamCursor, epoch_proof: Digest, prefix_digest: Digest },
}
impl StreamProgress {
    pub fn cursor(&self) -> &StreamCursor {
        match self {
            Self::LocalPending { cursor } | Self::Confirmed { cursor, .. } => cursor,
        }
    }
}
