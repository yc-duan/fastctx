//! A hub probes and reads only content qualified for this source operation.
use super::{SqliteBlocks, access, pins, rows, uploads};
use crate::{
    identity::OperationId,
    manifest::{ContentBlock, ContentRef},
    store::{Error, access::Custody, sql},
};

impl SqliteBlocks {
    pub(crate) fn source_upload_page(
        &self,
        root: &crate::manifest::ManifestRoot,
    ) -> Result<(crate::manifest::ManifestReply, super::budget::Permit), Error> {
        use fastctx_storage::rusqlite::{OptionalExtension, params};
        let length = self
            .database()
            .read_now(|db| {
                db.query_row(
                    "SELECT length(page) FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
                    params![root.0.domain.hub.to_string(), root.0.domain.scope.as_str(), root.0.digest.as_str()],
                    |r| r.get::<_, i64>(0),
                )
                .optional()
                .map_err(Into::into)
            })
            .map_err(|e| sql::error(e, self.worker().clone()))?
            .ok_or_else(|| Error::Unavailable("adopted source manifest is missing locally".into()))?;
        let length = usize::try_from(length).map_err(|_| Error::Corrupt("invalid stored source page length".into()))?;
        if length > fastctx_types::sync::PAGE_BYTES as usize {
            return Err(Error::Corrupt("stored source page exceeds its byte bound".into()));
        }
        // Admission covers the retained page, decoded references and their work rows.
        let permit = self.buffer_budget().try_acquire(length * 8 + 16384)?;
        let page = self.stored_page(root)?.ok_or_else(|| Error::Unavailable("source manifest disappeared".into()))?;
        if page.page_json.len() != length {
            return Err(Error::Corrupt("source manifest length changed".into()));
        }
        Ok((page, permit))
    }
    pub(crate) fn source_content_ready(
        &self,
        custody: &Custody,
        operation: &OperationId,
        root: &ContentRef,
    ) -> Result<bool, Error> {
        root.validate().map_err(Error::Invalid)?;
        if root.id().domain != custody.object.scope {
            return Err(Error::Denied("source content crosses its object scope".into()));
        }
        self.database()
            .read_now(|db| {
                pins::authorize(db, self.holder(), self.policy().as_ref(), custody)?;
                let qualified = if access::has_grant(db, custody, root)? {
                    true
                } else {
                    uploads::active_supply(db, custody, operation)?.is_some_and(|active| active.source.is_none())
                        && pins::ready_for(db, operation, custody, root)?
                };
                Ok(qualified && rows::complete(db, root)?)
            })
            .map_err(|e| sql::error(e, operation.clone()))
    }

    pub(crate) fn source_page_bytes(
        &self,
        custody: &Custody,
        operation: &OperationId,
        page: &ContentBlock,
    ) -> Result<(Vec<u8>, super::budget::Permit), Error> {
        if page.decoded_bytes > fastctx_types::sync::PAGE_BYTES {
            return Err(Error::Invalid("source manifest page exceeds its metadata bound".into()));
        }
        let root = ContentRef::from(page.clone());
        if !self.source_content_ready(custody, operation, &root)? {
            return Err(Error::Denied("manifest bytes were not supplied for this source operation".into()));
        }
        let value = self
            .stored_block(page)?
            .ok_or_else(|| Error::Unavailable("source manifest bytes are not present".into()))?;
        if !self.source_content_ready(custody, operation, &root)? {
            return Err(Error::Revoked);
        }
        Ok(value)
    }
}
