//! Bounded SQL metadata operations; file IO is always outside the writer.
use super::{location::Location, pins};
use crate::{
    identity::OperationId,
    manifest::{BlobId, ContentBlock, Descriptor, Envelope, Representation},
    store::sql,
};
use fastctx_storage::{
    Error, ErrorKind, Result,
    rusqlite::{Connection, OptionalExtension, params, types::Type},
};

pub(super) struct Stored {
    pub representation: Representation,
    pub location: Location,
    pub verified: bool,
}

pub(super) fn reserve(db: &Connection, value: Representation, operation: OperationId) -> Result<Stored> {
    let id = &value.content.id;
    let claimed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_asset_gc WHERE hub=?1 AND scope=?2 AND hash=?3 AND root_kind='block' AND claimed=1)",
        params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
        |row| row.get(0),
    )?;
    if claimed {
        return Err(Error::with_kind(ErrorKind::Busy, "content is being collected"));
    }
    let envelope = serde_json::to_vec(&value.envelope).map_err(|error| sql::invalid(error.to_string()))?;
    let length = db
        .query_row(
            "SELECT decoded_bytes FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3",
            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
            |row| row.get::<_, u32>(0),
        )
        .optional()?;
    if length.is_some_and(|length| length != value.content.decoded_bytes) {
        return Err(sql::invalid("content identity has conflicting decoded lengths"));
    }
    db.execute("INSERT INTO sync_asset_blocks(hub,scope,hash,decoded_bytes,verified) VALUES(?1,?2,?3,?4,0) ON CONFLICT(hub,scope,hash) DO NOTHING",params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),value.content.decoded_bytes])?;
    let previous=db.query_row(
        "SELECT representation,envelope,encoded_digest,encoded_bytes,storage_key,verified FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3 AND envelope=?4",
        params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),envelope],|row|stored(row,value.content.clone())
    ).optional()?;
    let key = match previous {
        Some(found) if found.verified => return Ok(found),
        Some(found) => {
            // Only a hidden slot may be rewritten under the exclusive IO lease.
            // Reusing its physical generation leaves no unindexed partial file.
            let key = found.location.rebind(&value, operation).map_err(|error| sql::invalid(error.to_string()))?;
            db.execute("DELETE FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3 AND representation=?4 AND verified=0",params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),found.representation.id().as_str()])?;
            key
        }
        None => Location::new(&value, operation),
    };
    db.execute(
        "INSERT INTO sync_asset_representations(hub,scope,hash,representation,envelope,encoded_digest,encoded_bytes,storage_key,verified) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
        params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),value.id().as_str(),envelope,value.encoded_digest.as_str(),value.encoded_bytes,key.storage_key().map_err(|error|sql::invalid(error.to_string()))?]
    )?;
    Ok(Stored { representation: value, location: key, verified: false })
}

pub(super) fn publish(db: &Connection, value: &Stored) -> Result<()> {
    let id = &value.representation.content.id;
    let count=db.execute(
        "UPDATE sync_asset_representations SET verified=1 WHERE hub=?1 AND scope=?2 AND hash=?3 AND representation=?4 AND storage_key=?5",
        params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),value.representation.id().as_str(),value.location.storage_key().map_err(|error|sql::invalid(error.to_string()))?]
    )?;
    if count != 1 {
        return Err(sql::invalid("publication no longer owns its reserved storage location"));
    }
    db.execute(
        "UPDATE sync_asset_blocks SET verified=1 WHERE hub=?1 AND scope=?2 AND hash=?3",
        params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
    )?;
    Ok(())
}

pub(super) fn descriptor(db: &Connection, id: &BlobId) -> Result<Descriptor> {
    let length = db
        .query_row(
            "SELECT decoded_bytes FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3 AND verified=1",
            params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()],
            |row| row.get::<_, u32>(0),
        )
        .optional()?
        .ok_or_else(|| Error::with_kind(ErrorKind::Unavailable, "content block is not locally complete"))?;
    let content = ContentBlock { id: id.clone(), decoded_bytes: length };
    let mut query=db.prepare_cached("SELECT representation,envelope,encoded_digest,encoded_bytes,storage_key,verified FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3 AND verified=1 ORDER BY representation LIMIT 3")?;
    let values = query
        .query_map(params![id.domain.hub.to_string(), id.domain.scope.as_str(), id.digest.as_str()], |row| {
            stored(row, content.clone())
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    if values.len() > 2 {
        return Err(sql::invalid("selected v1 supports only raw and zstd representation slots"));
    }
    Ok(Descriptor { content, representations: values.into_iter().map(|stored| stored.representation).collect() })
}

pub(super) fn representation(db: &Connection, value: &Representation) -> Result<Stored> {
    let id = &value.content.id;
    db.query_row(
        "SELECT representation,envelope,encoded_digest,encoded_bytes,storage_key,verified FROM sync_asset_representations WHERE hub=?1 AND scope=?2 AND hash=?3 AND representation=?4 AND verified=1",
        params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),value.id().as_str()],|row|stored(row,value.content.clone())
    ).optional()?.ok_or_else(||Error::with_kind(ErrorKind::Unavailable,"encoded representation is not locally complete"))
}

pub(super) fn stored(
    row: &fastctx_storage::rusqlite::Row<'_>,
    content: ContentBlock,
) -> fastctx_storage::rusqlite::Result<Stored> {
    let parse = |index: usize, error: String| {
        fastctx_storage::rusqlite::Error::FromSqlConversionFailure(
            index,
            Type::Blob,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
        )
    };
    let encoded: Vec<u8> = row.get(1)?;
    let envelope: Envelope = serde_json::from_slice(&encoded).map_err(|error| parse(1, error.to_string()))?;
    let value = Representation {
        content,
        envelope,
        encoded_digest: crate::identity::Digest::try_from(row.get::<_, String>(2)?).map_err(|error| parse(2, error))?,
        encoded_bytes: row.get(3)?,
    };
    value.validate().map_err(|error| parse(1, error))?;
    if value.id().as_str() != row.get::<_, String>(0)? {
        return Err(parse(0, "encoded representation identity mismatch".into()));
    }
    let key = Location::decode(&row.get::<_, String>(4)?, &value).map_err(|error| parse(4, error.to_string()))?;
    Ok(Stored { representation: value, location: key, verified: row.get(5)? })
}

pub(super) fn complete(db: &Connection, root: &crate::manifest::ContentRef) -> Result<bool> {
    let id = root.id();
    let (form, length) = pins::shape(root);
    match form {
        "block"=>db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_blocks WHERE hub=?1 AND scope=?2 AND hash=?3 AND decoded_bytes=?4 AND verified=1)",params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str(),length],|row|row.get(0)).map_err(Into::into),
        _=>db.query_row("SELECT EXISTS(SELECT 1 FROM sync_asset_manifests WHERE hub=?1 AND scope=?2 AND hash=?3 AND complete=1)",params![id.domain.hub.to_string(),id.domain.scope.as_str(),id.digest.as_str()],|row|row.get(0)).map_err(Into::into),
    }
}
