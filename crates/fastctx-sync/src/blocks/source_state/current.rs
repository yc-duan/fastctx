//! State content is usable only while its exact persisted checkpoint use is current.
use super::*;
use crate::store::roots::{RootLease, RootUse};

pub(super) fn checked(
    db: &Connection,
    store: &SqliteBlocks,
    target: &SourceTarget,
) -> SqlResult<Option<(Revision, Option<ContentRef>)>> {
    let custody = &target.custody;
    pins::authorize(db, &store.holder, store.policy.as_ref(), custody)?;
    let scope = &custody.object.scope;
    let row = db
        .query_row(
            "SELECT revision,state,state_lease FROM sync_sources WHERE hub=?1 AND scope=?2 AND source=?3",
            params![scope.hub.to_string(), scope.scope.as_str(), target.source.as_str()],
            |row| {
                Ok((
                    Revision(sql::counter(row, 0)?),
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .optional()?;
    row.map(|(revision, state, lease)| {
        let decode =
            |bytes: &[u8]| serde_json::from_slice::<ContentRef>(bytes).map_err(|error| sql::invalid(error.to_string()));
        let root = state.as_deref().map(decode).transpose()?;
        if let Some(root) = &root {
            let bytes = lease.ok_or_else(|| sql::invalid("checkpoint state has no persisted use"))?;
            let lease: RootLease = serde_json::from_slice(&bytes).map_err(|error| sql::invalid(error.to_string()))?;
            if lease.root != *root || lease.usage != (RootUse::Checkpoint { source: target.source.clone() }) {
                return Err(sql::invalid("checkpoint state differs from its owning use"));
            }
            crate::blocks::root_holds::retained(db, custody, &lease)?;
        }
        Ok((revision, root))
    })
    .transpose()
}
