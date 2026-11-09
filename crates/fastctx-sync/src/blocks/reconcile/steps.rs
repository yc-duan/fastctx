//! A closed fan-out step drains across all destinations independently of any scope scan.
use super::*;
use crate::{identity::SourceId, store::source_step::StepKey};

pub(super) fn invalidate_page(
    db: &Connection,
    current: &HoldingPeriod,
    after: Option<&SourceId>,
) -> SqlResult<state::Progress> {
    let scope = &current.scope;
    let rows=db.prepare_cached("SELECT target_source,scope,source,step,holding FROM sync_source_step_targets INDEXED BY sync_source_step_targets_active WHERE hub=?1 AND target_scope=?2 AND active=1 AND target_source>?3 ORDER BY target_source LIMIT 64")?.query_map(
        params![scope.hub.to_string(),scope.scope.as_str(),after.map_or("",SourceId::as_str)],
        |row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,sql::counter(row,3)?,row.get::<_,Vec<u8>>(4)?)),
    )?.collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let full = rows.len() == 64;
    let mut last = None;
    for (target, primary, source, revision, holding) in rows {
        let target = SourceId::try_from(target).map_err(sql::invalid)?;
        let old: HoldingPeriod = state::decode(&holding)?;
        if old.holder != current.holder || old.scope != *scope {
            return Err(sql::invalid("source target belongs to another holder or scope"));
        }
        last = Some(target);
        if old == *current {
            continue;
        }
        let key = StepKey {
            scope: ScopeKey {
                hub: scope.hub.clone(),
                scope: crate::identity::ScopeId::try_from(primary).map_err(sql::invalid)?,
            },
            source: SourceId::try_from(source).map_err(sql::invalid)?,
            revision: Revision(revision),
        };
        super::super::steps::retire(db, &scope.hub, &key)?;
        crate::scheduler::source_cleanup(db, &key)?;
        return Ok(state::Progress::Steps { after: last });
    }
    Ok(if full { state::Progress::Steps { after: last } } else { state::Progress::Uploads { after: None } })
}

pub(super) async fn process(blocks: &SqliteBlocks, lease: Lease) -> Result<Outcome, Error> {
    let suffix = lease
        .task
        .key
        .key
        .strip_prefix("source-step/")
        .ok_or_else(|| Error::Invalid("invalid source cleanup key".into()))?;
    let (source, revision) =
        suffix.split_once('/').ok_or_else(|| Error::Invalid("invalid source cleanup coordinates".into()))?;
    let key = StepKey {
        scope: lease.task.key.scope.clone(),
        source: SourceId::try_from(source.to_owned()).map_err(Error::Invalid)?,
        revision: Revision(revision.parse().map_err(|_| Error::Invalid("invalid source cleanup revision".into()))?),
    };
    let holder = blocks.holder().clone();
    blocks
        .database()
        .write(256 * 1024, move |db| {
            let _boundary = boundary::enter_typed_write(Role::Reconcile);
            let row = db
                .query_row(
                    "SELECT holding,state FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4",
                    params![
                        key.scope.hub.to_string(),
                        key.scope.scope.as_str(),
                        key.source.as_str(),
                        key.revision.0.to_be_bytes().as_slice()
                    ],
                    |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;
            let Some((holding, status)) = row else {
                return Ok(Outcome::Superseded);
            };
            let holding: HoldingPeriod = state::decode(&holding)?;
            if holding.holder != holder || status == "active" {
                return Err(sql::invalid("source cleanup does not own a closed step"));
            }
            Ok(if super::super::steps::drain_step(db, &key, 64)? == 0 {
                Outcome::Completed { through: lease.task.wanted }
            } else {
                Outcome::Retry { next_due_millis: now(), reason: "remaining closed source uploads".into() }
            })
        })
        .await
        .map_err(|error| sql::error(error, blocks.worker().clone()))
}
