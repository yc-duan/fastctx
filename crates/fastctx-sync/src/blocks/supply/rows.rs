//! Bounded durable work is resumed by key; a completed cursor never rescans a tree.
use super::model::{Request, SupplyProgress, SupplyTicket, Work, decode, encode};
use crate::{
    cursor::Revision,
    identity::{Digest, ScopeKey},
    store::sql,
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, OptionalExtension, params},
};

pub(super) struct Check {
    pub ticket: SupplyTicket,
    pub request: Request,
    pub supply_revision: Revision,
    pub phase: String,
    pub progress: SupplyProgress,
    pub next_work: u64,
}

pub(super) fn get(db: &Connection, ticket: &SupplyTicket) -> Result<Option<Check>> {
    let scope = &ticket.scope;
    let raw = db.query_row(
        "SELECT request,supply_revision,phase,progress,next_work FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4",
        params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice()],
        |row| Ok((row.get::<_,Vec<u8>>(0)?,sql::counter(row,1)?,row.get::<_,String>(2)?,row.get::<_,Vec<u8>>(3)?,sql::counter(row,4)?)),
    ).optional()?;
    raw.map(|(request, revision, phase, progress, next_work)| {
        Ok(Check {
            ticket: ticket.clone(),
            request: decode(&request)?,
            supply_revision: Revision(revision),
            phase,
            progress: decode(&progress)?,
            next_work,
        })
    })
    .transpose()
}
pub(super) fn latest(db: &Connection, scope: &ScopeKey, id: Digest) -> Result<Option<Check>> {
    let generation = db.query_row(
        "SELECT generation FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND id=?3 ORDER BY generation DESC LIMIT 1",
        params![scope.hub.to_string(),scope.scope.as_str(),id.as_str()],|row|sql::counter(row,0),
    ).optional()?;
    generation
        .map(|value| get(db, &SupplyTicket { scope: scope.clone(), id, generation: Revision(value) }))
        .transpose()
        .map(Option::flatten)
}
pub(super) fn create(
    db: &Connection,
    request: Request,
    generation: Revision,
    supply_revision: Revision,
) -> Result<Check> {
    let ticket = SupplyTicket { scope: request.object.scope.clone(), id: request.identity()?, generation };
    let progress = SupplyProgress::Pending { missing: None };
    let scope = &ticket.scope;
    db.execute(
        "INSERT INTO sync_supply_checks(hub,scope,id,generation,object,kind,operation,request,supply_revision,phase,progress,next_work,cleanup) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'walking',?10,?11,0)",
        params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),generation.0.to_be_bytes().as_slice(),request.object.object.as_str(),request.kind,request.operation.as_str(),encode(&request)?,supply_revision.0.to_be_bytes().as_slice(),encode(&progress)?,1_u64.to_be_bytes().as_slice()],
    )?;
    put_work(db, &ticket, 0, &Work::root(&request))?;
    Ok(Check { ticket, request, supply_revision, phase: "walking".into(), progress, next_work: 1 })
}
pub(super) fn progress(db: &Connection, check: &mut Check, phase: &str, value: SupplyProgress) -> Result<()> {
    let terminal = matches!(phase, "paused" | "rejected" | "cancelled" | "consumed");
    let scope = &check.ticket.scope;
    db.execute(
        "UPDATE sync_supply_checks SET phase=?1,progress=?2,next_work=?3,cleanup=?4 WHERE hub=?5 AND scope=?6 AND id=?7 AND generation=?8",
        params![phase,encode(&value)?,check.next_work.to_be_bytes().as_slice(),terminal,scope.hub.to_string(),scope.scope.as_str(),check.ticket.id.as_str(),check.ticket.generation.0.to_be_bytes().as_slice()],
    )?;
    check.phase = phase.into();
    check.progress = value;
    Ok(())
}
pub(super) fn first_work(db: &Connection, ticket: &SupplyTicket) -> Result<Option<(u64, Work)>> {
    let scope = &ticket.scope;
    let raw = db.query_row(
        "SELECT position,body FROM sync_supply_work WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4 ORDER BY position LIMIT 1",
        params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice()],
        |row|Ok((sql::counter(row,0)?,row.get::<_,Vec<u8>>(1)?)),
    ).optional()?;
    raw.map(|(at, body)| Ok((at, decode(&body)?))).transpose()
}
pub(super) fn put_work(db: &Connection, ticket: &SupplyTicket, at: u64, work: &Work) -> Result<()> {
    let body = encode(work)?;
    if body.len() > fastctx_types::sync::PAGE_BYTES as usize {
        return Err(sql::invalid("supply frontier row exceeds its metadata budget"));
    }
    let scope = &ticket.scope;
    db.execute(
        "INSERT INTO sync_supply_work(hub,scope,id,generation,position,body) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(hub,scope,id,generation,position) DO UPDATE SET body=excluded.body",
        params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice(),at.to_be_bytes().as_slice(),body],
    )?;
    Ok(())
}
pub(super) fn remove_work(db: &Connection, ticket: &SupplyTicket, at: u64) -> Result<()> {
    let scope = &ticket.scope;
    db.execute(
        "DELETE FROM sync_supply_work WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4 AND position=?5",
        params![
            scope.hub.to_string(),
            scope.scope.as_str(),
            ticket.id.as_str(),
            ticket.generation.0.to_be_bytes().as_slice(),
            at.to_be_bytes().as_slice()
        ],
    )?;
    Ok(())
}
