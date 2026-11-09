//! A source receipt acknowledges durable scoped facts, separately from body transfer.
use super::{
    catalog::ObjectHead,
    cursor::{ScopeCursor, StreamCursor},
    identity::{Digest, OperationId},
    proof::{AcceptedActor, ConfirmedEpoch},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReceipt {
    pub operation: OperationId,
    pub actor: AcceptedActor,
    pub proof: Digest,
    pub epoch: ConfirmedEpoch,
    pub stream: StreamCursor,
    pub prefix_digest: Digest,
    pub head: ObjectHead,
    /// Publication position, not evidence that a peer installed the preceding feed.
    pub scope: ScopeCursor,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SourceAccepted {
    /// The receipt is immutable; completeness observes this reply's retained content.
    Accepted {
        receipt: Box<SourceReceipt>,
        content_complete: bool,
    },
    AwaitingEpoch {
        proof: Digest,
    },
}
