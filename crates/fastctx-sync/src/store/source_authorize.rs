//! Signatures identify a producer; semantic authority still comes from this gate.
use super::{
    access::Decision,
    ingest::{SourceCheckpoint, SourceTarget},
    ingest_effects::SourceEffects,
};
use crate::catalog::{SourceBatch, SourceRecord};

/// The engine has verified these bounded bytes against the record's payload root.
pub struct SourcePayload<'a> {
    pub record: &'a SourceRecord,
    pub bytes: &'a [u8],
}
pub struct PreparedSourceChange<'a> {
    pub target: &'a SourceTarget,
    pub batch: Option<&'a SourceBatch>,
    pub payloads: &'a [SourcePayload<'a>],
    pub checkpoint: &'a SourceCheckpoint,
    pub effects: &'a SourceEffects,
}
/// Installed per kind. Called inside the local writer and again by the accepting
/// authority using its own trusted in-memory state. No IO or raw SQL is exposed.
/// Missing installation disables ingestion and capability advertisement, including
/// checkpoint-only writes. Hub audit/identity facts and device connection facts
/// have different source rules even though both carry valid source signatures.
pub trait SourceAuthorizer: Send + Sync {
    fn kind(&self) -> &'static str;
    fn authorize(&self, change: PreparedSourceChange<'_>) -> Decision;
}
