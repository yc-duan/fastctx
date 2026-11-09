//! A claimed generation cannot be reused while its physical files are removed.
use super::SqliteBlocks;
use crate::{
    blocks::{files::FileKey, root_holds, rows},
    identity::{Digest, HubId, ScopeId, ScopeKey},
    manifest::{BlobId, ContentBlock, DigestAlgorithm},
    store::{
        Error,
        boundary::{self, Role},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

#[derive(Clone)]
struct Candidate {
    id: BlobId,
    form: String,
}
struct Claimed {
    candidate: Candidate,
    files: Vec<FileKey>,
}

pub(super) async fn collect(store: &SqliteBlocks, max: u32) -> std::result::Result<u32, Error> {
    if max == 0 || max > 128 {
        return Err(Error::Invalid("garbage collection needs a page of 1 to 128 contents".into()));
    }
    let now = now();
    store
        .db
        .write(256 * 1024, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Content);
            root_holds::expire(db, now, max)
        })
        .await
        .map_err(|error| sql::error(error, store.worker.clone()))?;
    let candidates=store.db.read(move |db| {
        let mut query=db.prepare_cached("SELECT hub,scope,hash,root_kind FROM sync_asset_gc WHERE next_due<=?1 ORDER BY next_due,hub,scope,hash,root_kind LIMIT ?2")?;
        let raw=query.query_map(params![now,max],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,String>(3)?)))?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
        raw.into_iter().map(|(hub,scope,hash,form)|Ok(Candidate {id:BlobId {domain:ScopeKey {hub:HubId::try_from(hub).map_err(sql::invalid)?,scope:ScopeId::try_from(scope).map_err(sql::invalid)?},algorithm:DigestAlgorithm::Sha256,digest:Digest::try_from(hash).map_err(sql::invalid)?},form})).collect::<Result<Vec<_>>>()
    }).await.map_err(|error|sql::error(error,store.worker.clone()))?;
    let mut collected = 0;
    for candidate in candidates {
        let _lease = match store.leases.cleanup(&candidate.id) {
            Ok(lease) => lease,
            Err(Error::Busy) => {
                store.db.write(4096,move |db| {
                    let _boundary=boundary::enter_typed_write(Role::Content);
                    db.execute("UPDATE sync_asset_gc SET next_due=?1 WHERE hub=?2 AND scope=?3 AND hash=?4 AND root_kind=?5",params![now.saturating_add(50),candidate.id.domain.hub.to_string(),candidate.id.domain.scope.as_str(),candidate.id.digest.as_str(),candidate.form])?;Ok(())
                }).await.map_err(|error|sql::error(error,store.worker.clone()))?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let claimed = store
            .db
            .write(16384, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Content);
                claim(db, candidate)
            })
            .await
            .map_err(|error| sql::error(error, store.worker.clone()))?;
        let Some(claimed) = claimed else { continue };
        for file in &claimed.files {
            store.files.delete(file)?;
        }
        store
            .db
            .write(512 * 1024, move |db| {
                let _boundary = boundary::enter_typed_write(Role::Content);
                finish(db, &claimed.candidate)
            })
            .await
            .map_err(|error| sql::error(error, store.worker.clone()))?;
        collected += 1;
    }
    Ok(collected)
}

fn claim(db: &Connection, candidate: Candidate) -> Result<Option<Claimed>> {
    let id = &candidate.id;
    let hub = id.domain.hub.to_string();
    let scope = id.domain.scope.as_str();
    let hash = id.digest.as_str();
    let claimed = db
        .query_row(
            "SELECT claimed FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4",
            params![hub, scope, hash, candidate.form],
            |row| row.get::<_, bool>(0),
        )
        .optional()?;
    let Some(claimed) = claimed else { return Ok(None) };
    if !claimed {
        let retained:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_roots WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4) OR EXISTS(SELECT 1 FROM sync_asset_pins WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4 AND phase!='released') OR EXISTS(SELECT 1 FROM sync_asset_edges WHERE hub=?1 AND scope=?2 AND child=?3 AND child_kind=?4) OR EXISTS(SELECT 1 FROM sync_pending_roots WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4) OR EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4 AND released=0)",params![hub,scope,hash,candidate.form],|row|row.get(0))?;
        if retained {
            db.execute(
                "DELETE FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4",
                params![hub, scope, hash, candidate.form],
            )?;
            return Ok(None);
        }
        db.execute(
            "UPDATE sync_asset_gc SET claimed=1 WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4",
            params![hub, scope, hash, candidate.form],
        )?;
    }
    let mut files = Vec::new();
    if candidate.form == "block" {
        let length = db
            .query_row(
                "SELECT decoded_bytes FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3",
                params![hub, scope, hash],
                |row| row.get::<_, u32>(0),
            )
            .optional()?;
        if let Some(length) = length {
            let content = ContentBlock { id: id.clone(), decoded_bytes: length };
            let mut query=db.prepare_cached("SELECT representation,envelope,encoded_digest,encoded_bytes,storage_key,verified FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3 ORDER BY representation LIMIT 3")?;
            files = query
                .query_map(params![hub, scope, hash], |row| rows::stored(row, content.clone()))?
                .map(|row| row.map(|stored| stored.key))
                .collect::<fastctx_storage::rusqlite::Result<_>>()?;
            if files.len() > 2 {
                return Err(sql::invalid("garbage representation page exceeds v1 encodings"));
            }
        }
        db.execute(
            "UPDATE sync_asset_blocks SET verified=0 WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![hub, scope, hash],
        )?;
        db.execute(
            "UPDATE sync_asset_representations SET verified=0 WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![hub, scope, hash],
        )?;
    } else {
        db.execute(
            "UPDATE sync_asset_manifests SET complete=0 WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![hub, scope, hash],
        )?;
    }
    Ok(Some(Claimed { candidate, files }))
}

fn finish(db: &Connection, candidate: &Candidate) -> Result<()> {
    let id = &candidate.id;
    let hub = id.domain.hub.to_string();
    let scope = id.domain.scope.as_str();
    let hash = id.digest.as_str();
    let claimed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4 AND claimed=1)",params![hub,scope,hash,candidate.form],|row|row.get(0))?;
    if !claimed {
        return Err(sql::invalid("garbage collection lost its durable claim"));
    }
    if candidate.form == "block" {
        db.execute(
            "DELETE FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![hub, scope, hash],
        )?;
        db.execute("DELETE FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3", params![hub, scope, hash])?;
    } else {
        db.execute("INSERT INTO sync_asset_gc(hub,scope,hash,root_kind,generation,next_due) SELECT hub,scope,child,child_kind,?4,0 FROM sync_asset_edges WHERE hub=?1 AND scope=?2 AND parent=?3 ON CONFLICT(hub,scope,hash,root_kind) DO NOTHING",params![hub,scope,hash,1_u64.to_be_bytes().as_slice()])?;
        db.execute("DELETE FROM sync_asset_edges WHERE hub=?1 AND scope=?2 AND parent=?3", params![hub, scope, hash])?;
        db.execute(
            "DELETE FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![hub, scope, hash],
        )?;
    }
    db.execute(
        "DELETE FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind=?4",
        params![hub, scope, hash, candidate.form],
    )?;
    Ok(())
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
}
