//! Only verified remote facts introduce replica roots. They need no local upload
//! proof: immutable parent pages extend the same authenticated declaration.
use super::{SqliteBlocks, access, pins};
use crate::{
    manifest::ContentRef,
    store::{access::Custody, sql},
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};

pub(crate) fn authorize(db: &Connection, blocks: &SqliteBlocks, custody: &Custody) -> Result<()> {
    pins::authorize(db, blocks.holder(), blocks.policy().as_ref(), custody)
}
pub(crate) fn shape(root: &ContentRef) -> (&'static str, Option<u32>) {
    pins::shape(root)
}

/// The caller installs the owning stream/version root in this same transaction.
/// No business handle can call this path or relabel its own staging as evidence.
pub(crate) fn adopt(db: &Connection, custody: &Custody, root: &ContentRef) -> Result<()> {
    root.validate().map_err(sql::invalid)?;
    if root.id().domain != custody.object.scope {
        return Err(sql::invalid("replica root belongs to another scope"));
    }
    access::grant_root(db, &custody.object, custody.kind.registration().id, root, &custody.holding)?;
    enqueue(db, custody, root)
}
pub(super) fn enqueue(db: &Connection, custody: &Custody, root: &ContentRef) -> Result<()> {
    let scope = &custody.object.scope;
    let (form, length) = pins::shape(root);
    let holding = serde_json::to_vec(&custody.holding).map_err(|e| sql::invalid(e.to_string()))?;
    db.execute("INSERT INTO sync_replica_content(hub,scope,object,kind,hash,root_kind,decoded_bytes,holding,done) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0) ON CONFLICT(hub,scope,object,kind,hash,root_kind) DO UPDATE SET holding=excluded.holding",params![scope.hub.to_string(),scope.scope.as_str(),custody.object.object.as_str(),custody.kind.registration().id,root.id().digest.as_str(),form,length,holding])?;
    Ok(())
}
