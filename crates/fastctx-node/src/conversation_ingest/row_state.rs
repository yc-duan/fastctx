//! Small local consumption state; native block addresses live in registered rows,
//! never in a growing checkpoint vector. Public bodies are held by source facts.
use fastctx_hosts::claude::ingest::{
    document::{Address, Span},
    semantic::children::Children,
};
use fastctx_types::sync::{
    conversation_timeline::Placement,
    cursor::{ByteCount, Count, Sequence},
    identity::{Digest, ObjectId},
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Clock {
    pub accepted: u64,
    pub created: u64,
    pub turns: u64,
}
impl Clock {
    pub fn accept(&mut self) -> Result<Sequence, fastctx_sync::store::Error> {
        increment(&mut self.accepted)
    }
    pub fn create(&mut self) -> Result<Sequence, fastctx_sync::store::Error> {
        increment(&mut self.created)
    }
    pub fn turn(&mut self) -> Result<Sequence, fastctx_sync::store::Error> {
        increment(&mut self.turns)
    }
}
fn increment(n: &mut u64) -> Result<Sequence, fastctx_sync::store::Error> {
    *n =
        n.checked_add(1).ok_or_else(|| fastctx_sync::store::Error::Invalid("canonical allocation exhausted".into()))?;
    Ok(Sequence(*n))
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub created: Sequence,
    pub placement: Option<Placement>,
    pub display: ContentRef,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct NativeRow {
    pub body: Digest,
    pub turn: Option<ObjectId>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub turn: ObjectId,
}
#[derive(Clone, Serialize, Deserialize)]
pub enum Stage {
    Index,
    Turn,
    Content,
    Finish,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Work {
    pub start: ByteCount,
    pub stage: Stage,
    pub children: Option<Children>,
    pub total: Count,
    pub next: Count,
    pub images: Count,
    pub index_nested: Option<(Count, Children)>,
    pub first: Option<Span>,
    pub visible: bool,
    pub has_result: bool,
    pub original_published: bool,
    pub prompt_emitted: bool,
    pub placement: Option<Placement>,
    pub nested: Option<Nested>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Nested {
    pub parent: Count,
    pub address: Address,
    pub next: Count,
    pub total: Count,
    pub images: Count,
}
impl Work {
    pub fn new(start: ByteCount) -> Self {
        Self {
            start,
            stage: Stage::Index,
            children: None,
            total: Count(0),
            next: Count(0),
            images: Count(0),
            index_nested: None,
            first: None,
            visible: false,
            has_result: false,
            original_published: false,
            prompt_emitted: false,
            placement: None,
            nested: None,
        }
    }
}
pub const CLOCK: &str = "conversation-order-v1";

/// Mutable planning state for one semantic unit, never a database transaction.
pub(super) struct Local<'a, 'source> {
    pub rows: &'a mut super::indexes::Indexes<'source>,
    pub clock: &'a mut Clock,
    pub work: &'a mut Work,
}
impl<'a, 'source> Local<'a, 'source> {
    pub fn borrow(rows: &'a mut super::indexes::Indexes<'source>, clock: &'a mut Clock, work: &'a mut Work) -> Self {
        Self { rows, clock, work }
    }
}
