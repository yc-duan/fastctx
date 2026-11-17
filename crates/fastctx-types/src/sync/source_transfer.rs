//! Source staging is scoped to one original batch and the authenticated device.
use super::{
    cursor::HoldingPeriod,
    identity::{ObjectKey, OperationId, ProducerEpoch},
    transfer::KindVersion,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTransfer {
    pub holding: HoldingPeriod,
    pub object: ObjectKey,
    pub kind: KindVersion,
    pub epoch: ProducerEpoch,
    pub operation: OperationId,
}
