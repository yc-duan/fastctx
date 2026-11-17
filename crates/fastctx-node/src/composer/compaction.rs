//! Checkpoints are durable jobs submitted with the updates they will replace.

use fastctx_jobs::{Job, Jobs, Kind, Outcome, Taken};
use fastctx_storage::{
    Database, Result, documents,
    rusqlite::{Connection, params},
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};

pub(super) const KIND: &str = "composer/compact";

fn job(key: &documents::Key) -> Job {
    let mut hash = Sha256::new();
    for part in [&key.namespace, &key.owner, &key.name] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    Job {
        kind: KIND,
        key: format!("{:x}", hash.finalize()),
        unit: key.owner.clone(),
        payload: key.name.as_bytes().to_vec(),
    }
}

pub(super) fn job_bytes(key: &documents::Key) -> usize {
    KIND.len() + 64 + key.owner.len() + key.name.len() + 64
}

/// Records one coalesced task in the same transaction as its document updates.
pub(super) fn submit(db: &Connection, key: &documents::Key) -> Result<()> {
    fastctx_jobs::submit(db, &job(key))
}

pub(crate) fn register(node: &Arc<crate::Node>) {
    register_jobs(&node.durable_jobs, node.database.clone());
}

pub(super) fn register_jobs(jobs: &Jobs, database: Database) {
    jobs.register(
        Kind { name: KIND, at_once: 2, within: Duration::from_secs(60), tries: 5 },
        Arc::new(move |taken: Taken| {
            let database = database.clone();
            Box::pin(async move {
                let name = match String::from_utf8(taken.payload) {
                    Ok(name) => name,
                    Err(error) => return Outcome::Failed(error.to_string()),
                };
                let key = documents::Key { namespace: "composer".into(), owner: taken.unit, name };
                match compact_document(&database, key).await {
                    Ok(true) => Outcome::Done,
                    Ok(false) => Outcome::Again { after: Duration::from_millis(10) },
                    Err(error) => Outcome::Failed(error.to_string()),
                }
            })
        }),
    );
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
        .write(key.bytes() + checkpoint.len(), move |db| {
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
