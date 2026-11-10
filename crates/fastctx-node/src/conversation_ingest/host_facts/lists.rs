//! Reverse native scans preserve ordinals while publishing balanced public parts.
use super::part_tree::Builder;
use crate::conversation_ingest::step_content::StepContent;
use fastctx_sync::store::Error;
use fastctx_types::sync::{conversation_timeline::FactPartV1, cursor::Count, manifest::ContentRef};
pub struct List<'a, 'source>(Builder<'a, 'source>);
impl<'a, 'source> List<'a, 'source> {
    pub fn new(io: &'a StepContent<'source>) -> Self {
        Self(Builder::new(io, None, true))
    }
    pub async fn prepend(&mut self, index: Count, part: FactPartV1) -> Result<(), Error> {
        self.0.push(index, part).await
    }
    pub async fn finish(self) -> Result<Option<ContentRef>, Error> {
        self.0.finish().await
    }
}
