//! Decoded reader buffers retain ordinary object custody, grants and admission.
use super::{SqliteBlocks, access, pins};
use crate::blocks::budget::Permit;
use crate::{
    manifest::{ContentBlock, ContentRef, ManifestReply, ManifestRoot},
    store::{Error, access::Custody, sql},
};
impl SqliteBlocks {
    fn content_granted(&self, custody: &Custody, content: &ContentRef) -> Result<(), Error> {
        let granted = self
            .db
            .read_now(|db| {
                pins::authorize_read(db, &self.holder, self.policy.as_ref(), custody)?;
                access::has_grant(db, custody, content)
            })
            .map_err(|error| sql::error(error, self.worker.clone()))?;
        if granted {
            Ok(())
        } else {
            Err(Error::Unavailable("the owned content's object grant has not arrived".into()))
        }
    }
    pub(crate) fn content_block(&self, custody: &Custody, block: &ContentBlock) -> Result<(Vec<u8>, Permit), Error> {
        let content = ContentRef::from(block.clone());
        self.content_granted(custody, &content)?;
        let result =
            self.stored_block(block)?.ok_or_else(|| Error::Unavailable("the owned block has not arrived".into()))?;
        self.content_granted(custody, &content)?;
        Ok(result)
    }
    pub(crate) fn content_manifest(&self, custody: &Custody, root: &ManifestRoot) -> Result<ManifestReply, Error> {
        let content = ContentRef::Manifest { root: root.clone() };
        self.content_granted(custody, &content)?;
        let result =
            self.stored_page(root)?.ok_or_else(|| Error::Unavailable("the owned manifest has not arrived".into()))?;
        self.content_granted(custody, &content)?;
        Ok(result)
    }
}
