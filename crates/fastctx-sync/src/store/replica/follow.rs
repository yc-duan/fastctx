//! Durable following state is colocated with the receiver cursor. Network drivers
//! cannot advance it except through the receiver's verified commits.
use super::{
    SqliteReplica,
    rows::{self, decode, encode},
};
use crate::{
    cursor::{HoldingPeriod, Revision, ScopeCursor, Sequence},
    identity::{Digest, FeedEpoch, ScopeId, ScopeKey, StreamId},
    store::{
        Error, FutureResult,
        boundary::{self, Role},
        replica::ReplicaContext,
        scopes, sql,
    },
    transfer::ViewDefinition,
};
use fastctx_storage::{
    Result as SqlResult,
    rusqlite::{Connection, OptionalExtension, params},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FollowStop {
    Revoked,
    Denied { message: String },
    HoldingChanged,
}
#[derive(Clone, Debug)]
pub struct FollowScope {
    pub holding: HoldingPeriod,
    pub cursor: ScopeCursor,
    pub view: ViewDefinition,
    pub generation: Revision,
    pub stopped: Option<FollowStop>,
}
impl FollowScope {
    pub fn context(&self) -> ReplicaContext {
        ReplicaContext {
            source: crate::identity::Producer::Hub,
            holding: self.holding.clone(),
            source_uses: Default::default(),
        }
    }
}
#[derive(Clone, Debug)]
pub struct StreamNeed {
    pub stream: StreamId,
    pub epoch_proof: Digest,
}

impl SqliteReplica {
    pub(crate) fn follow_holding(&self, scope: ScopeKey) -> FutureResult<'_, Option<HoldingPeriod>> {
        Box::pin(async move {
            if scope.hub != self.hub.hub {
                return Err(Error::Denied("scope belongs to another hub".into()));
            }
            let holder = self.blocks.holder().clone();
            self.blocks
                .database()
                .read(move |db| {
                    Ok(scopes::current(db, &scope)?.filter(|s| s.enabled).map(|current| HoldingPeriod {
                        scope,
                        holder,
                        acl_revision: current.acl_revision,
                        publication_epoch: current.publication_epoch,
                    }))
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// One current receiver state, including a durable stop reason even after
    /// local custody is withdrawn. It grants neither reading nor retaining rights.
    pub fn follow_scope(&self, scope: ScopeKey) -> FutureResult<'_, Option<FollowScope>> {
        Box::pin(async move {
            if scope.hub != self.hub.hub {
                return Err(Error::Denied("scope belongs to another hub".into()));
            }
            self.blocks
                .database()
                .read(move |db| load(db, &scope))
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Keyset enumeration, including stopped scopes for diagnostics. The driver
    /// skips stopped entries rather than retrying a known denial indefinitely.
    pub fn follow_scopes(&self, after: Option<ScopeId>, max: u32) -> FutureResult<'_, Vec<FollowScope>> {
        Box::pin(async move {
            if max == 0 || max > 64 {
                return Err(Error::Invalid("following pages hold 1 to 64 scopes".into()));
            }
            let hub = self.hub.hub.clone();
            self.blocks
                .database()
                .read(move |db| {
                    let snapshot = db.unchecked_transaction()?;
                    let keys = snapshot
                        .prepare_cached(
                            "SELECT scope FROM sync_replica_scopes WHERE hub=?1 AND scope>?2 ORDER BY scope LIMIT ?3",
                        )?
                        .query_map(params![hub.to_string(), after.as_ref().map_or("", ScopeId::as_str), max], |row| {
                            row.get::<_, String>(0)
                        })?
                        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
                    let mut found = Vec::new();
                    for key in keys {
                        let scope = ScopeKey { hub: hub.clone(), scope: ScopeId::try_from(key).map_err(sql::invalid)? };
                        found.push(load(&snapshot, &scope)?.ok_or_else(|| sql::invalid("tracked scope disappeared"))?);
                    }
                    snapshot.commit()?;
                    Ok(found)
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Atomically starts replay at the authenticated hub's new feed genesis.
    /// Installed facts and operation receipts remain; only page receipts change
    /// generation, so a prior identical page cannot stand in for the new commit.
    pub fn restart_follow(&self, expected: FollowScope, epoch: FeedEpoch) -> FutureResult<'_, ()> {
        Box::pin(async move {
            self.connection(&expected.context())?;
            self.blocks
                .database()
                .write(16384, move |db| {
                    let _role = boundary::enter_typed_write(Role::Replica);
                    rows::current(db, &expected.holding)?;
                    let current =
                        load(db, &expected.holding.scope)?.ok_or_else(|| sql::invalid("scope is not tracked"))?;
                    if current.generation != expected.generation
                        || current.cursor != expected.cursor
                        || current.holding != expected.holding
                    {
                        return Err(fastctx_storage::Error::with_kind(
                            fastctx_storage::ErrorKind::Busy,
                            "another follower advanced before reset",
                        ));
                    }
                    let generation = current
                        .generation
                        .0
                        .checked_add(1)
                        .ok_or_else(|| sql::invalid("follow generation exhausted"))?;
                    let cursor = ScopeCursor {
                        epoch: epoch.clone(),
                        through: Sequence(0),
                        prefix_digest: Some(fastctx_types::sync::feed::genesis(&current.cursor.scope, &epoch)),
                        ..current.cursor
                    };
                    db.execute(
                        "UPDATE sync_replica_scopes SET generation=?3,cursor=?4 WHERE hub=?1 AND scope=?2",
                        params![
                            cursor.scope.hub.to_string(),
                            cursor.scope.scope.as_str(),
                            generation.to_be_bytes().as_slice(),
                            encode(&cursor)?
                        ],
                    )?;
                    retire_pages(db, &cursor.scope, Revision(generation))?;
                    Ok(())
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// A delayed denial from an older tracking generation cannot stop a new one.
    /// This stops transport work; the authenticated directory and existing scope
    /// reconciler own permission changes and physical retirement.
    pub fn stop_follow(&self, expected: FollowScope, mut reason: FollowStop) -> FutureResult<'_, ()> {
        Box::pin(async move {
            self.connection(&expected.context())?;
            if let FollowStop::Denied { message } = &mut reason {
                *message = message.chars().take(512).collect();
            }
            self.blocks.database().write(8192, move |db| {
                let _role = boundary::enter_typed_write(Role::Replica);
                db.execute("UPDATE sync_replica_scopes SET following='stopped',stop_reason=?5 WHERE hub=?1 AND scope=?2 AND generation=?3 AND holding=?4", params![expected.holding.scope.hub.to_string(), expected.holding.scope.scope.as_str(), expected.generation.0.to_be_bytes().as_slice(), encode(&expected.holding)?, encode(&reason)?])?;
                Ok(())
            }).await.map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Finds work left by a committed feed page, including missing predecessor
    /// descriptions. This is independent of what this process last downloaded.
    pub fn next_stream(&self, context: ReplicaContext) -> FutureResult<'_, Option<StreamNeed>> {
        Box::pin(async move {
            self.connection(&context)?;
            self.blocks.database().read(move |db| {
                rows::current(db, &context.holding)?;
                let scope = &context.holding.scope;
                db.prepare_cached("SELECT stream,epoch_proof FROM sync_replica_streams WHERE hub=?1 AND scope=?2 AND holding=?3 AND pending=1 ORDER BY stream,epoch_proof LIMIT 1")?
                    .query_row(params![scope.hub.to_string(), scope.scope.as_str(), encode(&context.holding)?], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
                    .optional()?.map(|(stream, proof)| Ok(StreamNeed { stream: StreamId::try_from(stream).map_err(sql::invalid)?, epoch_proof: Digest::try_from(proof).map_err(sql::invalid)? })).transpose()
            }).await.map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    /// Clears only work whose durable source cursor reached the current target,
    /// with its complete predecessor chain and no waiting page.
    pub fn finish_stream(&self, context: ReplicaContext, need: StreamNeed) -> FutureResult<'_, bool> {
        Box::pin(async move {
            self.connection(&context)?;
            let blocks = self.blocks.clone();
            self.blocks.database().write(8192, move |db| {
                let _role = boundary::enter_typed_write(Role::Replica);
                let target = super::stream_rows::target(db, &blocks, &context, &need.stream, &need.epoch_proof)?;
                let Some(epoch) = super::stream_rows::epoch(db, &context.holding.scope, &need.stream, &need.epoch_proof)? else { return Ok(false); };
                let (through, _, _) = super::stream_rows::head(db, &target, &epoch, &need.epoch_proof)?;
                if through < target.wanted { return Ok(false); }
                let scope = &context.holding.scope;
                let waiting: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_waiting WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4 AND holding=?5) OR EXISTS(SELECT 1 FROM sync_replica_epoch_fences f WHERE f.hub=?1 AND f.scope=?2 AND f.stream=?3 AND NOT EXISTS(SELECT 1 FROM sync_epoch_descriptors e WHERE e.hub=f.hub AND e.scope=f.scope AND e.stream=f.stream AND e.epoch=f.epoch))", params![scope.hub.to_string(), scope.scope.as_str(), need.stream.as_str(), need.epoch_proof.as_str(), encode(&context.holding)?], |row| row.get(0))?;
                if waiting { return Ok(false); }
                db.execute("UPDATE sync_replica_streams SET pending=0 WHERE hub=?1 AND scope=?2 AND stream=?3 AND epoch_proof=?4", params![scope.hub.to_string(), scope.scope.as_str(), need.stream.as_str(), need.epoch_proof.as_str()])?;
                Ok(true)
            }).await.map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
    pub(crate) fn retire_follow_pages(&self, state: FollowScope) -> FutureResult<'_, ()> {
        Box::pin(async move {
            self.connection(&state.context())?;
            let check = state.clone();
            let pending: bool = self
                .blocks
                .database()
                .read(move |db| {
                    Ok(db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM sync_replica_pages WHERE hub=?1 AND scope=?2 AND generation<?3)",
                        params![
                            check.holding.scope.hub.to_string(),
                            check.holding.scope.scope.as_str(),
                            check.generation.0.to_be_bytes().as_slice()
                        ],
                        |row| row.get(0),
                    )?)
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))?;
            if !pending {
                return Ok(());
            }
            self.blocks
                .database()
                .write(16384, move |db| {
                    let _role = boundary::enter_typed_write(Role::Replica);
                    retire_pages(db, &state.holding.scope, state.generation)
                })
                .await
                .map_err(|e| sql::error(e, self.blocks.worker().clone()))
        })
    }
}
fn retire_pages(db: &Connection, scope: &ScopeKey, generation: Revision) -> SqlResult<()> {
    db.execute("DELETE FROM sync_replica_pages WHERE (hub,scope,generation,holding,page) IN (SELECT hub,scope,generation,holding,page FROM sync_replica_pages WHERE hub=?1 AND scope=?2 AND generation<?3 ORDER BY generation,holding,page LIMIT 64)", params![scope.hub.to_string(), scope.scope.as_str(), generation.0.to_be_bytes().as_slice()])?;
    Ok(())
}
fn load(db: &Connection, scope: &ScopeKey) -> SqlResult<Option<FollowScope>> {
    let row = db
        .query_row(
            "SELECT holding,cursor,view,generation,stop_reason FROM sync_replica_scopes WHERE hub=?1 AND scope=?2",
            params![scope.hub.to_string(), scope.scope.as_str()],
            |row| {
                Ok((
                    row.get::<_, Vec<u8>>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    sql::counter(row, 3)?,
                    row.get::<_, Option<Vec<u8>>>(4)?,
                ))
            },
        )
        .optional()?;
    row.map(|(holding, cursor, view, generation, stopped)| {
        Ok(FollowScope {
            holding: decode(&holding)?,
            cursor: decode(&cursor)?,
            view: decode(&view)?,
            generation: Revision(generation),
            stopped: stopped.map(|bytes| decode(&bytes)).transpose()?,
        })
    })
    .transpose()
}
