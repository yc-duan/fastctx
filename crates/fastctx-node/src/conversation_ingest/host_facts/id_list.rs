//! Incremental typed UUID lists never put a source-sized vector in a checkpoint.
use super::{
    content,
    part_tree::{Builder, State},
};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::{Address, Value, read::Reader},
    semantic::children::Children,
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{
    conversation_identity::NativeId, conversation_timeline::*, cursor::Count, manifest::ContentRef,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct IdList {
    children: Option<Children>,
    count: Count,
    tree: Option<State>,
}
impl IdList {
    pub async fn new(reader: &Reader<'_>, address: Address) -> Result<Self, Error> {
        let children = Children::of(reader, address)
            .await
            .map_err(super::error)?
            .ok_or_else(|| super::invalid("native UUID list is not an array"))?;
        Ok(Self { children: Some(children), count: Count(0), tree: None })
    }
    pub async fn advance(&mut self, io: &StepContent<'_>, reader: &Reader<'_>) -> Result<(ContentRef, bool), Error> {
        let mut builder = match self.tree.take() {
            Some(state) => Builder::resume(io, state),
            None => Builder::new(io, Some(FactPartKindV1::NativeId), true),
        };
        if let Some(children) = &mut self.children {
            for _ in 0..16 {
                let Some((_, node)) = children.step(reader).await.map_err(super::error)? else { break };
                let Value::String { text } = node.value else {
                    return Err(super::invalid("native UUID is not a string"));
                };
                let value = reader
                    .text(text, 4096)
                    .await
                    .map_err(super::error)?
                    .ok_or_else(|| super::invalid("native UUID exceeds its identity bound"))?;
                builder
                    .push(node.ordinal, FactPartV1::NativeId { value: NativeId::new(value).map_err(super::invalid)? })
                    .await?;
                self.count.0 = self.count.0.checked_add(1).ok_or(Error::Busy)?;
            }
            if children.remaining.0 > 0 {
                let state = builder.checkpoint().await?;
                let root = Builder::resume(io, state.clone()).finish().await?.ok_or(Error::Busy)?;
                self.tree = Some(state);
                return Ok((root, false));
            }
        }
        self.children = None;
        let reference = match builder.finish().await? {
            Some(reference) => reference,
            None => {
                content::publish(
                    io,
                    ItemFactV1::Parts { part_kind: FactPartKindV1::NativeId, start: Count(0), parts: vec![] },
                )
                .await?
            }
        };
        Ok((reference, true))
    }
}
