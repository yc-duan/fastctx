use crate::{
    cursor::{HoldingPeriod, Sequence},
    identity::{Digest, ProducerEpoch, ScopeKey, StreamId},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SourceUploadKey {
    pub scope: ScopeKey,
    pub stream: StreamId,
    pub epoch: ProducerEpoch,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SourceUploadStop {
    Revoked,
    HoldingChanged,
    NotPromoted,
    Denied { message: String },
    Rejected { message: String },
    Corrupt { message: String },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceUploadPhase {
    Idle,
    Content,
    Submit,
    Waiting,
    Confirm,
    Stopped,
}
impl SourceUploadPhase {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Content => "content",
            Self::Submit => "submit",
            Self::Waiting => "waiting",
            Self::Confirm => "confirm",
            Self::Stopped => "stopped",
        }
    }
    pub(super) fn parse(value: &str) -> fastctx_storage::Result<Self> {
        match value {
            "idle" => Ok(Self::Idle),
            "content" => Ok(Self::Content),
            "submit" => Ok(Self::Submit),
            "waiting" => Ok(Self::Waiting),
            "confirm" => Ok(Self::Confirm),
            "stopped" => Ok(Self::Stopped),
            _ => Err(crate::store::sql::invalid("unknown source upload phase")),
        }
    }
}
/// Durable remote progress, independent of the source's local checkpoint and
/// epoch confirmation. A stopped tenure is never silently rebound to a new one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceUploadState {
    pub key: SourceUploadKey,
    pub holding: HoldingPeriod,
    pub through: Sequence,
    pub prefix_digest: Option<Digest>,
    pub proof: Option<Digest>,

    pub phase: SourceUploadPhase,
    pub receipt: Option<crate::store::authority::SourceReceipt>,
    pub stopped: Option<SourceUploadStop>,
}
