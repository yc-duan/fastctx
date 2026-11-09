//! Index payload IO occurs outside the writer; only verified structure is published.
use super::SqliteBlocks;
use crate::{
    blocks::{budget::Permit, codec, manifest::CheckedManifest, metadata, pages, rows},
    cursor::{HoldingPeriod, LocalSequence},
    identity::{Digest, OperationId},
    manifest::{BlobId, ContentBlock, ContentRef, DigestAlgorithm, ManifestPage, ManifestRoot},
    scheduler::{self, Outcome},
    store::{
        Error,
        access::Custody,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::rusqlite::params;

pub(super) async fn process(
    store: &SqliteBlocks,
    holding: HoldingPeriod,
    hash: Digest,
    through: LocalSequence,
    after: Option<&str>,
) -> Result<Outcome, Error> {
    let root = ManifestRoot(BlobId {
        domain: holding.scope.clone(),
        algorithm: DigestAlgorithm::Sha256,
        digest: hash.clone(),
    });
    let _lease = store.leases.read(&root.0)?;
    let page=store.db.read_now(|db| {
        if !super::work::current(db,&holding)? {return Ok(None);}
        let needed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=0) AND NOT EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='manifest' AND claimed=1)",params![holding.scope.hub.to_string(),holding.scope.scope.as_str(),hash.as_str()],|r|r.get(0))?;
        if needed {pages::get(db,&root).map(Some)} else {Ok(None)}
    }).map_err(|e|sql::error(e,store.worker.clone()))?;
    let Some(page) = page else {
        return Ok(Outcome::Completed { through });
    };
    let ManifestPage::Metadata { codec: kind, payload, references } =
        serde_json::from_str(&page.page_json).map_err(|e| Error::Corrupt(format!("invalid stored index page: {e}")))?
    else {
        return Err(Error::Corrupt("unvalidated page has no registered index codec".into()));
    };
    let (custody, next) = store
        .db
        .read_now(|db| super::metadata_custody::current(db, store, &holding, &root, &payload, after))
        .map_err(|e| sql::error(e, store.worker.clone()))?;
    if let Some(next) = next {
        let holding = holding.clone();
        store
            .db
            .write(4096, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Content);
                if super::work::current(db, &holding)? {
                    scheduler::content_changed(db, &holding, format!("metadata:{}:{next}", hash.as_str()))?;
                }
                Ok(())
            })
            .await
            .map_err(|e| sql::error(e, store.worker.clone()))?;
        return Ok(Outcome::Completed { through });
    }
    let Some(validation) = custody else {
        return Ok(Outcome::Blocked {
            dependency: format!(
                "index-custody:{}:{}:{}",
                holding.scope.hub,
                holding.scope.scope.as_str(),
                hash.as_str()
            ),
            reason: "waiting for authorized index payload bytes".into(),
        });
    };
    use crate::metrics::{Cause, Counter, Metrics, PlatformMetrics};
    PlatformMetrics.add(Counter::IndexValidation, Cause::Integrity, 1);
    let payload = validation.payload().clone();
    let Some((decoded, _buffer)) = payload_bytes(store, &payload, page.page_json.len() * 4)? else {
        return Ok(Outcome::Blocked {
            dependency: format!(
                "block:{}:{}:{}",
                holding.scope.hub,
                holding.scope.scope.as_str(),
                payload.id.digest.as_str()
            ),
            reason: "waiting for the index payload".into(),
        });
    };
    metadata::validate(&holding.scope, kind, &payload, &decoded, &references)?;
    let policy = store.policy.clone();
    let holder = store.holder.clone();
    store
        .db
        .write(16384, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Content);
            if !super::work::current(db, &holding)? {
                return Ok(Outcome::Superseded);
            }
            crate::blocks::pins::authorize(db, &holder, policy.as_ref(), validation.custody())?;
            let hash = &validation.root().0.digest;
            let changed = db.execute(
                "UPDATE sync_asset_manifests SET validated=1 WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=0",
                params![holding.scope.hub.to_string(), holding.scope.scope.as_str(), hash.as_str()],
            )?;
            if changed != 0 {
                pages::refresh(db, &holding.scope, hash)?;
                scheduler::content_changed(db, &holding, format!("complete:{}", hash.as_str()))?;
                scheduler::content_changed(db, &holding, format!("access:{}", hash.as_str()))?;
            }
            Ok(Outcome::Completed { through })
        })
        .await
        .map_err(|e| sql::error(e, store.worker.clone()))
}

/// Reads a verified payload's decoded bytes under an IO lease, or None while the
/// payload is not stored. The permit covers the bytes plus the caller's `extra`
/// working memory and must live as long as they do.
pub(super) fn payload_bytes(
    store: &SqliteBlocks,
    payload: &ContentBlock,
    extra: usize,
) -> Result<Option<(Vec<u8>, Permit)>, Error> {
    let _payload = store.leases.read(&payload.id)?;
    let stored = store
        .db
        .read_now(|db| {
            let available: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3 AND verified=1)",
                params![
                    payload.id.domain.hub.to_string(),
                    payload.id.domain.scope.as_str(),
                    payload.id.digest.as_str()
                ],
                |r| r.get(0),
            )?;
            if !available {
                return Ok(None);
            }
            let descriptor = rows::descriptor(db, &payload.id)?;
            if descriptor.content != *payload {
                return Err(sql::invalid("index payload descriptor differs from its declared extent"));
            }
            let representation = descriptor
                .representations
                .first()
                .ok_or_else(|| sql::invalid("index payload has no stored representation"))?;
            rows::representation(db, representation).map(Some)
        })
        .map_err(|e| sql::error(e, store.worker.clone()))?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let buffer = store.budget.acquire(
        stored.representation.encoded_bytes as usize
            + payload.decoded_bytes as usize
            + extra
            + codec::decode_workspace(stored.representation.envelope.encoding),
    )?;
    let encoded = store.files.read(&stored.key, stored.representation.encoded_bytes)?;
    Ok(Some((codec::decode(&stored.representation, encoded)?, buffer)))
}

/// Validates a metadata page published after its payload by the same operation,
/// so the page is complete when it is stored instead of after a worker pass.
/// Returns false when that payload was not supplied first; a source step
/// (`required`) must supply it, a replica publication waits for the worker.
pub(super) fn validate_supplied(
    store: &SqliteBlocks,
    custody: &Custody,
    operation: &OperationId,
    checked: &CheckedManifest,
    required: bool,
) -> Result<bool, Error> {
    let page = &checked.reply().page_json;
    let ManifestPage::Metadata { codec: kind, payload, references } =
        serde_json::from_str(page).map_err(|e| Error::Invalid(format!("invalid metadata page: {e}")))?
    else {
        return Err(Error::Invalid("only a metadata page carries a payload to validate".into()));
    };
    let supplied = store
        .db
        .read_now(|db| crate::blocks::pins::ready_for(db, operation, custody, &ContentRef::from(payload.clone())))
        .map_err(|e| sql::error(e, store.worker.clone()))?;
    if !supplied {
        return if required {
            Err(Error::Invalid("publish a metadata page's payload through the same step first".into()))
        } else {
            Ok(false)
        };
    }
    let Some((decoded, _buffer)) = payload_bytes(store, &payload, page.len() * 4)? else {
        return Err(Error::Invalid("a metadata page's payload is not stored".into()));
    };
    metadata::validate(&custody.object.scope, kind, &payload, &decoded, &references)?;
    Ok(true)
}
