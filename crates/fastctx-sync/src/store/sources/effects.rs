//! A destination's registered effects: its index families, the projections it
//! marks stale at this commit's local sequence, and its non-checkpoint root uses.
use crate::{
    blocks::source_commit,
    cursor::{LocalSequence, Revision},
    scheduler,
    store::{
        ingest::SourceTarget,
        ingest_effects::{IndexValue, SourceEffects, SourceIndexChange},
        mutation::Expected,
        roots::RootLease,
        source_step::SourceStep,
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

fn revision(db: &Connection, target: &SourceTarget, change: &SourceIndexChange) -> Result<Option<Revision>> {
    let scope = &target.custody.object.scope;
    db.prepare_cached(
        "SELECT revision FROM sync_source_index WHERE hub=?1 AND scope=?2 AND source=?3 AND family=?4 AND key=?5",
    )?
    .query_row(
        params![scope.hub.to_string(), scope.scope.as_str(), target.source.as_str(), change.family.id(), change.key],
        |row| sql::counter(row, 0).map(Revision),
    )
    .optional()
    .map_err(Into::into)
}

/// Every index precondition must hold before the commit writes anything.
pub(super) fn check(db: &Connection, target: &SourceTarget, effects: &SourceEffects) -> Result<()> {
    for change in effects.indexes() {
        let current = revision(db, target, change)?;
        let expected = match change.expected {
            Expected::Absent => None,
            Expected::Revision(revision) => Some(revision),
        };
        if current != expected {
            return Err(sql::invalid("a source index row moved from its expected revision"));
        }
        if current.is_none() && matches!(change.value, IndexValue::Delete) {
            return Err(sql::invalid("a source index deletion names an absent row"));
        }
    }
    Ok(())
}

pub(super) fn apply(
    db: &Connection,
    content: &source_commit::ContentCommit<'_>,
    step: &SourceStep,
    target: &SourceTarget,
    effects: &SourceEffects,
    local: LocalSequence,

    leases: &mut Vec<RootLease>,
) -> Result<()> {
    let scope = &target.custody.object.scope;
    let hub = scope.hub.to_string();
    for change in effects.indexes() {
        match &change.value {
            IndexValue::Put(value) => {
                let next = revision(db, target, change)?
                    .map_or(Some(1), |current| current.0.checked_add(1))
                    .ok_or_else(|| sql::invalid("source index revisions exhausted"))?;
                db.prepare_cached("INSERT INTO sync_source_index(hub,scope,source,family,key,revision,value) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(hub,scope,source,family,key) DO UPDATE SET revision=excluded.revision,value=excluded.value")?
                    .execute(params![
                        hub,
                        scope.scope.as_str(),
                        target.source.as_str(),
                        change.family.id(),
                        change.key,
                        next.to_be_bytes().as_slice(),
                        value
                    ])?;
            }
            IndexValue::Delete => {
                db.prepare_cached(
                    "DELETE FROM sync_source_index WHERE hub=?1 AND scope=?2 AND source=?3 AND family=?4 AND key=?5",
                )?
                .execute(params![
                    hub,
                    scope.scope.as_str(),
                    target.source.as_str(),
                    change.family.id(),
                    change.key
                ])?;
            }
        }
    }
    for dirty in effects.dirty() {
        scheduler::projection_changed(db, dirty.kind.registration().id, &dirty.object, local)?;
    }
    for change in effects.roots() {
        if let Some(lease) = content.change_root(step, change)? {
            leases.push(lease);
        }
    }
    Ok(())
}
