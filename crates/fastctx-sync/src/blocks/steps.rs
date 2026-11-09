//! Engine-issued source steps: issue, bind destinations, cancel and reconcile.
//! Every function runs inside the caller's write transaction and stays bounded.
use super::{pins, uploads};
use crate::{
    cursor::{Holder, Revision},
    identity::{HostGeneration, HubId, ScopeKey, SourceId},
    registry::{ConflictRule, DataClass},
    store::{
        access::{AccessPolicy, HoldingUse},
        ingest::{IngestReceipt, SourceTarget},
        source_step::{SourceStep, StepKey},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

/// Destinations per step, the same bound as one ingestion operation.
pub(crate) const STEP_TARGETS: u32 = 256;

pub(crate) enum Cancelled {
    Draining { released: u32 },
    Done,
    Committed { receipt: Box<IngestReceipt> },
}

pub(crate) enum StepState {
    Active,
    Consumed(Box<IngestReceipt>),
    Cancelled,
}

struct Active {
    revision: Revision,
    generation: String,
    expected: Option<Revision>,
    holding: Vec<u8>,
}

fn encode(value: &impl serde::Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|error| sql::invalid(error.to_string()))
}
fn counter(value: Option<Vec<u8>>) -> Result<Option<Revision>> {
    value
        .map(|bytes| {
            <[u8; 8]>::try_from(bytes.as_slice())
                .map(|bytes| Revision(u64::from_be_bytes(bytes)))
                .map_err(|_| sql::invalid("invalid stored step revision"))
        })
        .transpose()
}

fn validate(target: &SourceTarget) -> Result<()> {
    let custody = &target.custody;
    let kind = custody.kind.registration();
    let scope = &custody.object.scope;
    if custody.context.scope != *scope
        || custody.holding.scope != *scope
        || kind.class != DataClass::Asset
        || kind.conflict != ConflictRule::SingleWriter
        || custody.purpose == HoldingUse::Replica
    {
        return Err(sql::invalid("a source target needs a single-writer asset kind and a source custody"));
    }
    Ok(())
}

pub(crate) fn committed(db: &Connection, scope: &ScopeKey, source: &SourceId) -> Result<Option<Revision>> {
    let revision = db
        .prepare_cached("SELECT revision FROM sync_sources WHERE hub=?1 AND scope=?2 AND source=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), source.as_str()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .optional()?;
    counter(revision)
}

fn active(db: &Connection, scope: &ScopeKey, source: &SourceId) -> Result<Option<Active>> {
    let row = db
        .prepare_cached("SELECT step,generation,expected,holding FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND state='active'")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), source.as_str()], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<Vec<u8>>>(2)?, row.get::<_, Vec<u8>>(3)?))
        })
        .optional()?;
    row.map(|(step, generation, expected, holding)| {
        Ok(Active {
            revision: counter(Some(step))?.ok_or_else(|| sql::invalid("missing step revision"))?,
            generation,
            expected: counter(expected)?,
            holding,
        })
    })
    .transpose()
}

/// Issues a step, or returns the identical active one after a restart. A
/// different active step is cancelled; its staged pins drain in later pages.
pub(crate) fn begin(
    db: &Connection,
    holder: &Holder,
    policy: &dyn AccessPolicy,
    primary: &SourceTarget,
    expected: Option<Revision>,
    generation: &HostGeneration,
) -> Result<SourceStep> {
    validate(primary)?;
    pins::authorize(db, holder, policy, &primary.custody)?;
    let scope = &primary.custody.object.scope;
    let committed = committed(db, scope, &primary.source)?;
    if committed != expected {
        return Err(sql::invalid("the source checkpoint is not at the expected revision"));
    }
    let holding = encode(&primary.custody.holding)?;
    if let Some(found) = active(db, scope, &primary.source)? {
        if found.generation == generation.as_str() && found.expected == expected && found.holding == holding {
            return Ok(SourceStep::new(primary.clone(), generation.clone(), expected, found.revision));
        }
        let stale = StepKey { scope: scope.clone(), source: primary.source.clone(), revision: found.revision };
        retire(db, &scope.hub, &stale)?;
    }
    let issued = db
        .prepare_cached("SELECT issued FROM sync_source_step_heads WHERE hub=?1 AND scope=?2 AND source=?3")?
        .query_row(params![scope.hub.to_string(), scope.scope.as_str(), primary.source.as_str()], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .optional()?;
    let floor = counter(issued)?.max(committed).map_or(0, |revision| revision.0);
    let revision = Revision(floor.checked_add(1).ok_or_else(|| sql::invalid("source step revisions exhausted"))?);
    db.prepare_cached("INSERT INTO sync_source_step_heads(hub,scope,source,issued) VALUES(?1,?2,?3,?4) ON CONFLICT(hub,scope,source) DO UPDATE SET issued=excluded.issued")?
        .execute(params![scope.hub.to_string(), scope.scope.as_str(), primary.source.as_str(), revision.0.to_be_bytes().as_slice()])?;
    let step = SourceStep::new(primary.clone(), generation.clone(), expected, revision);
    db.prepare_cached("INSERT INTO sync_source_steps(hub,scope,source,step,generation,expected,operation,holding,state) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'active')")?
        .execute(params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            primary.source.as_str(),
            revision.0.to_be_bytes().as_slice(),
            generation.as_str(),
            expected.map(|revision| revision.0.to_be_bytes().to_vec()),
            step.operation().as_str(),
            holding
        ])?;
    bind(db, holder, policy, &step, primary)?;
    Ok(step)
}

/// Binds one destination to an active step; binding the same one again is a no-op.
pub(crate) fn bind(
    db: &Connection,
    holder: &Holder,
    policy: &dyn AccessPolicy,
    step: &SourceStep,
    target: &SourceTarget,
) -> Result<()> {
    validate(target)?;
    let key = step.key();
    let hub = key.scope.hub.to_string();
    if target.custody.object.scope.hub != key.scope.hub {
        return Err(sql::invalid("a step cannot bind a destination in another hub"));
    }
    let revision = key.revision.0.to_be_bytes();
    let open: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND operation=?5 AND state='active')")?
        .query_row(params![hub, key.scope.scope.as_str(), key.source.as_str(), revision.as_slice(), step.operation().as_str()], |row| row.get(0))?;
    if !open {
        return Err(sql::invalid("the source step is no longer active"));
    }
    pins::authorize(db, holder, policy, &target.custody)?;
    let custody = &target.custody;
    let destination = &custody.object.scope;
    let holding = encode(&custody.holding)?;
    let purpose = encode(&custody.purpose)?;
    let found = db
        .prepare_cached("SELECT scope,source,step,object,kind,stream,holding,purpose FROM sync_source_step_targets WHERE hub=?1 AND target_scope=?2 AND target_source=?3 AND active=1")?
        .query_row(params![hub, destination.scope.as_str(), target.source.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Vec<u8>>(6)?,
                row.get::<_, Vec<u8>>(7)?,
            ))
        })
        .optional()?;
    if let Some((scope, source, held, object, kind, stream, saved_holding, saved_purpose)) = found {
        let same_step = scope == key.scope.scope.as_str() && source == key.source.as_str() && held == revision;
        if !same_step {
            return Err(sql::invalid("the destination serves another active source step"));
        }
        if object != custody.object.object.as_str()
            || kind != custody.kind.registration().id
            || stream != target.stream.as_str()
            || saved_holding != holding
            || saved_purpose != purpose
        {
            return Err(sql::invalid("a bound destination cannot change its object, stream or custody"));
        }
        return Ok(());
    }
    let count: u32 = db
        .prepare_cached(
            "SELECT COUNT(*) FROM sync_source_step_targets WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4",
        )?
        .query_row(params![hub, key.scope.scope.as_str(), key.source.as_str(), revision.as_slice()], |row| {
            row.get(0)
        })?;
    if count >= STEP_TARGETS {
        return Err(sql::invalid("a source step binds too many destinations"));
    }
    db.prepare_cached("INSERT INTO sync_source_step_targets(hub,scope,source,step,target_scope,target_source,object,kind,stream,holding,purpose,active) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,1)")?
        .execute(params![
            hub,
            key.scope.scope.as_str(),
            key.source.as_str(),
            revision.as_slice(),
            destination.scope.as_str(),
            target.source.as_str(),
            custody.object.object.as_str(),
            custody.kind.registration().id,
            target.stream.as_str(),
            holding,
            purpose
        ])?;
    Ok(())
}

/// The persisted state of `step`; a step that names another operation is refused.
pub(crate) fn state(db: &Connection, step: &SourceStep) -> Result<StepState> {
    let key = step.key();
    let found = db
        .prepare_cached(
            "SELECT operation,state,receipt FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4",
        )?
        .query_row(
            params![
                key.scope.hub.to_string(),
                key.scope.scope.as_str(),
                key.source.as_str(),
                key.revision.0.to_be_bytes().as_slice()
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, Option<Vec<u8>>>(2)?)),
        )
        .optional()?;
    let Some((operation, state, receipt)) = found else {
        return Err(sql::invalid("the source step does not exist"));
    };
    if operation != step.operation().as_str() {
        return Err(sql::invalid("the source step names another operation"));
    }
    match state.as_str() {
        "active" => Ok(StepState::Active),
        "consumed" => {
            let receipt = receipt.ok_or_else(|| sql::invalid("a consumed step lost its receipt"))?;
            let receipt = serde_json::from_slice(&receipt).map_err(|error| sql::invalid(error.to_string()))?;
            Ok(StepState::Consumed(Box::new(receipt)))
        }
        _ => Ok(StepState::Cancelled),
    }
}

/// The destination must still be bound to this step with the custody it bound.
pub(crate) fn require_bound(db: &Connection, step: &SourceStep, target: &SourceTarget) -> Result<()> {
    let key = step.key();
    let custody = &target.custody;
    let bound: bool = db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_source_step_targets WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND target_scope=?5 AND target_source=?6 AND object=?7 AND kind=?8 AND stream=?9 AND holding=?10 AND purpose=?11 AND active=1)")?
        .query_row(
            params![
                key.scope.hub.to_string(),
                key.scope.scope.as_str(),
                key.source.as_str(),
                key.revision.0.to_be_bytes().as_slice(),
                custody.object.scope.scope.as_str(),
                target.source.as_str(),
                custody.object.object.as_str(),
                custody.kind.registration().id,
                target.stream.as_str(),
                encode(&custody.holding)?,
                encode(&custody.purpose)?
            ],
            |row| row.get(0),
        )?;
    if !bound {
        return Err(sql::invalid("the destination is not bound to this source step"));
    }
    Ok(())
}

/// Closes a committed step with its receipt. Its uploads become consumed and its
/// staged pins drain in later pages, like a cancelled step's.
pub(crate) fn consume(db: &Connection, step: &SourceStep, receipt: &IngestReceipt) -> Result<()> {
    let key = step.key();
    let hub = key.scope.hub.to_string();
    let revision = key.revision.0.to_be_bytes();
    let changed = db
        .prepare_cached("UPDATE sync_source_steps SET state='consumed',receipt=?6 WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND operation=?5 AND state='active'")?
        .execute(params![hub, key.scope.scope.as_str(), key.source.as_str(), revision.as_slice(), step.operation().as_str(), encode(receipt)?])?;
    if changed != 1 {
        return Err(sql::invalid("the source step is no longer active"));
    }
    db.prepare_cached(
        "UPDATE sync_source_step_targets SET active=0 WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4",
    )?
    .execute(params![hub, key.scope.scope.as_str(), key.source.as_str(), revision.as_slice()])?;
    uploads::consume_step(db, &key.scope.hub, &key)
}

/// Closes an active step: no new upload or commit can use it afterwards.
fn retire(db: &Connection, hub: &HubId, key: &StepKey) -> Result<()> {
    let revision = key.revision.0.to_be_bytes();
    db.prepare_cached("UPDATE sync_source_steps SET state='cancelled' WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND state='active'")?
        .execute(params![hub.to_string(), key.scope.scope.as_str(), key.source.as_str(), revision.as_slice()])?;
    db.prepare_cached(
        "UPDATE sync_source_step_targets SET active=0 WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4",
    )?
    .execute(params![hub.to_string(), key.scope.scope.as_str(), key.source.as_str(), revision.as_slice()])?;
    uploads::cancel_step(db, hub, key)
}

/// Durably cancels the step if it is still active, then releases one page of
/// its staged pins. The persisted holder must match; the period may be old.
pub(crate) fn cancel(db: &Connection, holder: &Holder, step: &SourceStep, max: u32) -> Result<Cancelled> {
    let key = step.key();
    let hub = &key.scope.hub;
    pins::same_holder(holder, &step.primary().custody)?;
    let revision = key.revision.0.to_be_bytes();
    let found = db
        .prepare_cached("SELECT operation,holding,state,receipt FROM sync_source_steps WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4")?
        .query_row(params![hub.to_string(), key.scope.scope.as_str(), key.source.as_str(), revision.as_slice()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?, row.get::<_, String>(2)?, row.get::<_, Option<Vec<u8>>>(3)?))
        })
        .optional()?;
    let Some((operation, holding, state, receipt)) = found else {
        return Err(sql::invalid("the source step does not exist"));
    };
    let period: crate::cursor::HoldingPeriod =
        serde_json::from_slice(&holding).map_err(|error| sql::invalid(error.to_string()))?;
    if operation != step.operation().as_str() || &period.holder != holder {
        return Err(sql::invalid("the cancellation names another step or holder"));
    }
    match state.as_str() {
        "consumed" => {
            let receipt = receipt.ok_or_else(|| sql::invalid("a consumed step lost its receipt"))?;
            let receipt = serde_json::from_slice(&receipt).map_err(|error| sql::invalid(error.to_string()))?;
            return Ok(Cancelled::Committed { receipt: Box::new(receipt) });
        }
        "active" => retire(db, hub, &key)?,
        _ => {}
    }
    match uploads::release_step(db, hub, &key, max)? {
        0 => {
            mark_drained(db, hub, &key)?;
            Ok(Cancelled::Done)
        }
        released => Ok(Cancelled::Draining { released }),
    }
}

fn mark_drained(db: &Connection, hub: &HubId, key: &StepKey) -> Result<()> {
    db.prepare_cached("UPDATE sync_source_steps SET drained=1 WHERE hub=?1 AND scope=?2 AND source=?3 AND step=?4 AND state!='active'")?
        .execute(params![hub.to_string(), key.scope.scope.as_str(), key.source.as_str(), key.revision.0.to_be_bytes().as_slice()])?;
    Ok(())
}

/// Where an interrupted startup pass resumes: the last primary source examined.
pub type ReconcileCursor = (HubId, crate::identity::ScopeId, SourceId);

fn key(hub: String, scope: String, source: String, step: Vec<u8>) -> Result<StepKey> {
    Ok(StepKey {
        scope: ScopeKey {
            hub: HubId::try_from(hub).map_err(sql::invalid)?,
            scope: crate::identity::ScopeId::try_from(scope).map_err(sql::invalid)?,
        },
        source: SourceId::try_from(source).map_err(sql::invalid)?,
        revision: counter(Some(step))?.ok_or_else(|| sql::invalid("missing step revision"))?,
    })
}

/// One bounded startup page over active steps in primary-source order. A step
/// at or below its committed checkpoint, or whose primary object was deleted, is
/// cancelled; the one valid next step of a source stays for its adapter.
pub(crate) fn reconcile(db: &Connection, after: Option<&ReconcileCursor>, max: u32) -> Result<Option<ReconcileCursor>> {
    let (hub, scope, source) = after.map_or((String::new(), String::new(), String::new()), |(hub, scope, source)| {
        (hub.to_string(), scope.as_str().to_owned(), source.as_str().to_owned())
    });
    let open = db
        .prepare_cached("SELECT s.hub,s.scope,s.source,s.step,t.object FROM sync_source_steps s JOIN sync_source_step_targets t ON t.hub=s.hub AND t.scope=s.scope AND t.source=s.source AND t.step=s.step AND t.target_scope=s.scope AND t.target_source=s.source WHERE s.state='active' AND (s.hub,s.scope,s.source)>(?1,?2,?3) ORDER BY s.hub,s.scope,s.source LIMIT ?4")?
        .query_map(params![hub, scope, source, max], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    let full = open.len() as u32 == max;
    let mut next = None;
    for (hub, scope, source, step, object) in open {
        let step = key(hub, scope, source, step)?;
        let deleted: bool = db
            .prepare_cached("SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)")?
            .query_row(params![step.scope.hub.to_string(), step.scope.scope.as_str(), object], |row| row.get(0))?;
        let stale = committed(db, &step.scope, &step.source)?.is_some_and(|done| step.revision <= done);
        next = Some((step.scope.hub.clone(), step.scope.scope.clone(), step.source.clone()));
        if deleted || stale {
            retire(db, &step.scope.hub.clone(), &step)?;
        }
    }
    Ok(if full { next } else { None })
}

/// Releases one page of pins staged by closed steps; zero means nothing waits.
pub(crate) fn drain(db: &Connection, max: u32) -> Result<u32> {
    let mut released = 0;
    let closed = db
        .prepare_cached("SELECT hub,scope,source,step FROM sync_source_steps WHERE state!='active' AND drained=0 ORDER BY hub,scope,source,step LIMIT ?1")?
        .query_map(params![max], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, Vec<u8>>(3)?))
        })?
        .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
    for (hub, scope, source, step) in closed {
        if released >= max {
            break;
        }
        let step = key(hub, scope, source, step)?;
        let hub = step.scope.hub.clone();
        match uploads::release_step(db, &hub, &step, max - released)? {
            0 => mark_drained(db, &hub, &step)?,
            count => released += count,
        }
    }
    Ok(released)
}
