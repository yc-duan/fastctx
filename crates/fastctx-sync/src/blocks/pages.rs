//! Immutable pages and typed edges commit together; missing children stay pending.
use super::manifest::{CheckedManifest, Reference};
use crate::{
    identity::{Digest, ScopeKey},
    manifest::{BlobId, ContentRef, DigestAlgorithm, ManifestReply, ManifestRoot},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn put(db: &Connection, checked: &CheckedManifest) -> Result<()> {
    let id = &checked.reply.root.0;
    let claimed: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='manifest' AND claimed=1)",params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str()],|row|row.get(0))?;
    if claimed {
        return Err(fastctx_storage::Error::with_kind(fastctx_storage::ErrorKind::Busy, "manifest is being collected"));
    }
    let existing = db
        .query_row(
            "SELECT page FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing != checked.reply.page_json.as_bytes() {
            return Err(sql::invalid("manifest identity already has different page bytes"));
        }
        return Ok(());
    }
    db.execute(
        "INSERT INTO sync_asset_manifests(hub,scope,hash,page,complete,validated) VALUES(?1,?2,?3,?4,0,?5)",
        params![
            id.domain.hub.to_string(),
            id.domain.scope.as_str(),
            id.digest.as_str(),
            checked.reply.page_json.as_bytes(),
            !checked.metadata
        ],
    )?;
    for reference in &checked.references {
        let (form, length) = match reference {
            Reference::Block { decoded_bytes, .. } => ("block", Some(*decoded_bytes)),
            Reference::Manifest { .. } => ("manifest", None),
        };
        db.execute(
            "INSERT INTO sync_asset_edges(hub,scope,parent,child,child_kind,decoded_bytes) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                id.domain.hub.to_string(),
                id.domain.scope.as_str(),
                id.digest.as_str(),
                reference.id().digest.as_str(),
                form,
                length
            ],
        )?;
    }
    refresh(db, &id.domain, &id.digest)?;
    Ok(())
}

pub(super) fn get(db: &Connection, root: &ManifestRoot) -> Result<ManifestReply> {
    let id = &root.0;
    let bytes = db
        .query_row(
            "SELECT page FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .ok_or_else(|| sql::invalid("manifest page is not present"))?;
    if Digest::of(&bytes) != id.digest {
        return Err(sql::invalid("stored manifest page failed its content digest"));
    }
    Ok(ManifestReply {
        root: root.clone(),
        page_json: String::from_utf8(bytes).map_err(|error| sql::invalid(error.to_string()))?,
    })
}

pub(super) fn refresh(db: &Connection, scope: &ScopeKey, hash: &Digest) -> Result<bool> {
    let missing:bool=db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_edges AS e LEFT JOIN sync_asset_blocks AS b ON e.hub=b.hub AND e.scope=b.scope AND e.child=b.hash LEFT JOIN sync_asset_manifests AS m ON e.hub=m.hub AND e.scope=m.scope AND e.child=m.hash WHERE e.hub=?1 AND e.scope=?2 AND e.parent=?3 AND ((e.child_kind='block' AND (b.verified IS NOT 1 OR b.decoded_bytes IS NOT e.decoded_bytes)) OR (e.child_kind='manifest' AND m.complete IS NOT 1))) OR EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='manifest' AND claimed=1) OR EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND validated=0)",
        params![scope.hub.to_string(),scope.scope.as_str(),hash.as_str()],|row|row.get(0)
    )?;
    let changed = db.execute(
        "UPDATE sync_asset_manifests SET complete=?1 WHERE hub=?2 AND scope=?3 AND hash=?4 AND complete<>?1",
        params![!missing, scope.hub.to_string(), scope.scope.as_str(), hash.as_str()],
    )?;
    Ok(changed != 0)
}

pub(super) fn references(db: &Connection, scope: &ScopeKey, hash: &Digest) -> Result<Vec<ContentRef>> {
    let mut query=db.prepare_cached("SELECT child,child_kind,decoded_bytes FROM sync_asset_edges WHERE hub=?1 AND scope=?2 AND parent=?3 ORDER BY child,child_kind LIMIT 4097")?;
    let rows = query
        .query_map(params![scope.hub.to_string(), scope.scope.as_str(), hash.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<u32>>(2)?))
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    if rows.len() > 4096 {
        return Err(sql::invalid("stored manifest edges exceed the page bound"));
    }
    rows.into_iter()
        .map(|(hash, form, length)| {
            let id = BlobId {
                domain: scope.clone(),
                algorithm: DigestAlgorithm::Sha256,
                digest: Digest::try_from(hash).map_err(sql::invalid)?,
            };
            match form.as_str() {
                "block" => Ok(ContentRef::Single {
                    block: id,
                    decoded_bytes: length.ok_or_else(|| sql::invalid("block edge has no decoded size"))?,
                }),
                "manifest" => Ok(ContentRef::Manifest { root: ManifestRoot(id) }),
                _ => Err(sql::invalid("unknown manifest reference form")),
            }
        })
        .collect()
}
