//! Engine writers read content they or their peers already stored, such as the
//! pages of a timeline being extended, without a reader's custody: a caller's
//! own access is checked where the caller asks, not on these internal reads.
use super::{SqliteBlocks, pages};
use crate::blocks::budget::Permit;
use crate::{
    identity::{RootLeaseId, ScopeKey},
    manifest::{ContentBlock, ManifestReply, ManifestRoot},
    store::{Error, sql},
};
use fastctx_storage::rusqlite::{OptionalExtension, params};

impl SqliteBlocks {
    /// A stored manifest page, or None while it is not here.
    pub(crate) fn stored_page(&self, root: &ManifestRoot) -> Result<Option<ManifestReply>, Error> {
        let _lease = self.leases.read(&root.0)?;
        self.db
            .read_now(|db| {
                let present = db
                    .query_row(
                        "SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
                        params![root.0.domain.hub.to_string(), root.0.domain.scope.as_str(), root.0.digest.as_str()],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if present { pages::get(db, root).map(Some) } else { Ok(None) }
            })
            .map_err(|error| sql::error(error, self.worker.clone()))
    }
    /// A stored block's decoded bytes, or None while it is not here.
    pub(crate) fn stored_block(&self, block: &ContentBlock) -> Result<Option<(Vec<u8>, Permit)>, Error> {
        super::metadata_work::payload_bytes(self, block, 0)
    }
    /// Whether a root use is still held: neither released nor expired at `now`.
    pub(crate) fn lease_held(&self, scope: &ScopeKey, lease: &RootLeaseId, now: i64) -> Result<bool, Error> {
        self.db
            .read_now(|db| {
                db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3 AND released=0 AND (expires IS NULL OR expires>?4))",
                    params![scope.hub.to_string(), scope.scope.as_str(), lease.as_str(), now],
                    |row| row.get(0),
                )
                .map_err(Into::into)
            })
            .map_err(|error| sql::error(error, self.worker.clone()))
    }
}
