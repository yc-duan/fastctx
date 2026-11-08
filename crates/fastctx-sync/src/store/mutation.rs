//! A mutation carries its precondition, root references and idempotency key.

use crate::{
    cursor::{LocalSequence, Revision},
    identity::{ObjectId, OperationId, SourceId},
    manifest::BlobId,
};

#[derive(Clone, Debug)]
pub enum Expected {
    Absent,
    Revision(Revision),
}

#[derive(Clone, Debug)]
pub enum AssetChange {
    Put { content: BlobId, history: Option<BlobId>, schema: u16 },
    Remove,
    Delete,
}

#[derive(Clone, Debug)]
pub struct Mutation {
    pub operation: OperationId,
    pub object: ObjectId,
    pub expected: Expected,
    pub change: AssetChange,
}

#[derive(Clone, Debug)]
pub enum Origin {
    Local,
    Replicated { source: crate::identity::DeviceId },
    Imported { source: SourceId },
}

#[derive(Clone, Debug)]
pub struct RuntimeChange {
    pub operation: OperationId,
    pub key: String,
    pub expected: Expected,
    pub body: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct ProjectionChange {
    pub operation: OperationId,
    pub object: ObjectId,
    /// The asset input covered by this projection, committed with its cursor.
    pub source: LocalSequence,
    pub body: Vec<u8>,
}
