//! Provider usage relates one message to its native items, without summing rows.
use super::{
    content,
    part_tree::{Builder, State},
};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_hosts::claude::ingest::{
    document::read::Reader,
    semantic::{children::Children, fields, row::Header},
};
use fastctx_sync::store::Error;
use fastctx_types::sync::{
    conversation_identity::{NativeId, NativeItemKey},
    conversation_timeline::*,
    cursor::Count,
    manifest::ContentRef,
};
use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
pub struct Usage {
    children: Option<Children>,
    count: Count,
    tree: Option<State>,
}
impl Usage {
    pub async fn new(reader: &Reader<'_>, header: &Header) -> Result<Self, Error> {
        let children = match header.content {
            Some(at) => Children::of(reader, at).await.map_err(super::error)?,
            None => None,
        };
        Ok(Self { children, count: Count(0), tree: None })
    }
    pub async fn advance(
        &mut self,
        io: &StepContent<'_>,
        reader: &Reader<'_>,
        header: &Header,
    ) -> Result<(ContentRef, bool), Error> {
        let uuid = NativeId::new(header.uuid.as_deref().ok_or(Error::Busy)?).map_err(super::invalid)?;
        let mut builder = match self.tree.take() {
            Some(state) => Builder::resume(io, state),
            None => Builder::new(io, Some(FactPartKindV1::NativeItem), true),
        };
        if let Some(children) = &mut self.children {
            for _ in 0..16 {
                let Some((at, node)) = children.step(reader).await.map_err(super::error)? else { break };
                let id = if fields::text(reader, at, "type").await.map_err(super::error)?.as_deref() == Some("tool_use")
                {
                    fields::text(reader, at, "id")
                        .await
                        .map_err(super::error)?
                        .and_then(|value| NativeId::new(value).ok())
                } else {
                    None
                };
                let item = match id {
                    Some(tool_use_id) => NativeItemKey::ClaudeTool { tool_use_id },
                    None => NativeItemKey::ClaudeBlock { uuid: uuid.clone(), block: node.ordinal },
                };
                builder.push(node.ordinal, FactPartV1::NativeItem { item }).await?;
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
        let result = match builder.finish().await? {
            Some(reference) => reference,
            None => {
                content::publish(
                    io,
                    ItemFactV1::Parts { part_kind: FactPartKindV1::NativeItem, start: Count(0), parts: vec![] },
                )
                .await?
            }
        };
        Ok((result, true))
    }
}
