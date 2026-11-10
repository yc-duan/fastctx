//! Journal compaction is background work. Service composition installs a submitter
//! that records its job in the edit's transaction and wakes it after commit.

use fastctx_storage::{
    Database, Result, documents,
    rusqlite::{Connection, params},
};
use std::sync::Arc;

/// The job integration supplies a transaction-local, coalescing enqueue. The key
/// belongs to the composer namespace and one conversation/document.
pub type CompactionSubmitter = fn(&Connection, &documents::Key) -> Result<()>;

/// Hooks installed by the node's durable job service before it accepts editors.
#[derive(Clone)]
pub struct Compactor {
    pub submit: CompactionSubmitter,
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl crate::Node {
    /// Installs the composer's transaction-local job submitter and post-commit
    /// wakeup before accepting editors. Returns false when already installed.
    #[allow(dead_code, reason = "The node job-service composition installs this hook.")]
    pub(crate) fn install_composer_compactor(&self, compactor: Compactor) -> bool {
        self.composers.compactor.set(Arc::new(compactor)).is_ok()
    }
}

/// Reconstructs confirmed authority without composer locks or a writer lease.
/// False asks the worker to retry: an edit won the revision race, so the newer
/// tail remains intact. Repeating a completed compaction is harmless.
pub async fn compact_document(database: &Database, key: documents::Key) -> Result<bool> {
    if key.namespace != "composer" {
        return Err(fastctx_storage::Error::new("compaction requires a composer document"));
    }
    let read_key = key.clone();
    let loaded = database
        .read(move |db| {
            let transaction = db.unchecked_transaction()?;
            let loaded = super::persistence::journal::load(&transaction, &read_key)?.0;
            transaction.commit()?;
            Ok(loaded)
        })
        .await?;

    if loaded.updates.is_empty() {
        return Ok(true);
    }
    let revision = loaded.revision;
    let through = i64::try_from(revision - loaded.updates.len() as u64)
        .map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
    let revision = i64::try_from(revision).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
    let checkpoint = fastctx_platform::task::spawn_blocking(move || {
        let mut document = if loaded.checkpoint.is_empty() {
            fastctx_draft::Document::new()
        } else {
            fastctx_draft::Document::restore(&loaded.checkpoint)
                .map_err(|error| fastctx_storage::Error::new(error.to_string()))?
        };
        for update in loaded.updates {
            document.apply(&update).map_err(|error| fastctx_storage::Error::new(error.to_string()))?;
        }
        Ok::<_, fastctx_storage::Error>(document.checkpoint())
    })
    .await
    .map_err(|error| fastctx_storage::Error::new(error.to_string()))??;
    database
        .write_relaxed(key.bytes() + checkpoint.len(), move |db| {
            let changed = db.prepare_cached(
            "UPDATE documents SET through=?4 WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision=?4 AND through=?5"
        )?.execute(params![key.namespace,key.owner,key.name,revision,through])?;
            if changed == 0 {
                return Ok(false);
            }
            super::persistence::journal::checkpoint(db, &key, &checkpoint)?;
            db.prepare_cached(
                "DELETE FROM document_updates WHERE namespace=?1 AND owner=?2 AND name=?3 AND revision<=?4",
            )?
            .execute(params![key.namespace, key.owner, key.name, revision])?;
            Ok(true)
        })
        .await
}
