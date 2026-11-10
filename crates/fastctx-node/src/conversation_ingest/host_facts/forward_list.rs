//! Forward native scans retain one active leaf and a logarithmic carry stack.
use super::{content, part_tree::Builder};
use crate::conversation_ingest::step_content::StepContent;
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::*, cursor::Count, manifest::ContentRef};
pub struct ForwardList<'a, 'source> {
    builder: Builder<'a, 'source>,
    io: &'a StepContent<'source>,
    kind: FactPartKindV1,
    count: u64,
}
impl<'a, 'source> ForwardList<'a, 'source> {
    pub fn new(io: &'a StepContent<'source>, kind: FactPartKindV1) -> Self {
        Self { builder: Builder::new(io, Some(kind), false), io, kind, count: 0 }
    }
    pub async fn push(&mut self, part: FactPartV1) -> Result<(), Error> {
        self.builder.push(Count(self.count), part).await?;
        self.count = self.count.checked_add(1).ok_or(Error::Busy)?;
        Ok(())
    }
    pub async fn finish(self) -> Result<ContentRef, Error> {
        match self.builder.finish().await? {
            Some(reference) => Ok(reference),
            None => {
                content::publish(self.io, ItemFactV1::Parts { part_kind: self.kind, start: Count(0), parts: vec![] })
                    .await
            }
        }
    }
}
