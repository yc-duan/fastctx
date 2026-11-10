//! Historical feed replay preserves applied heads, tombstones and notifications.
use super::{heads, rows::encode};
use crate::{
    blocks::SqliteBlocks,
    catalog::CatalogChange,
    identity::Digest,
    store::{replica::ReplicaContext, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) fn install(
    db: &Connection,
    blocks: &SqliteBlocks,
    context: &ReplicaContext,
    change: &CatalogChange,
    accepted: Option<&heads::AcceptedEvidence>,
) -> Result<()> {
    let scope = &change.head.key.scope;
    let period = encode(&context.holding)?;
    let digest = Digest::of(&encode(&change.head)?);
    let previous: Option<String> = db.query_row(
        "SELECT head_digest FROM sync_replica_head_receipts WHERE hub=?1 AND scope=?2 AND holding=?3 AND object=?4 AND operation=?5",
        params![scope.hub.to_string(), scope.scope.as_str(), period, change.head.key.object.as_str(), change.operation.as_str()],
        |row| row.get(0),
    ).optional()?;
    if let Some(previous) = previous {
        if previous != digest.as_str() {
            return Err(sql::invalid("a replayed operation changed its accepted head"));
        }
        return Ok(());
    }
    // A custody denial is also a completed observation in this holding. If it
    // were omitted, replay could install that older head and then skip a newer
    // already-installed head, rolling the object back after Reset.
    heads::install(db, blocks, context, change, accepted)?;
    db.execute("INSERT INTO sync_replica_head_receipts(hub,scope,holding,object,operation,head_digest) VALUES(?1,?2,?3,?4,?5,?6)",
            params![scope.hub.to_string(), scope.scope.as_str(), period, change.head.key.object.as_str(), change.operation.as_str(), digest.as_str()])?;
    Ok(())
}
