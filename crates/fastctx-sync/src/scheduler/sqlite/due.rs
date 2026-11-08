use super::*;

// Persist the wheel so max_tasks=1 and repeated restarts cannot starve history.
const WHEEL: [u8; 8] = [0, 1, 0, 2, 0, 1, 0, 3];

pub(super) fn claim(
    db: &Connection,
    owner: &OperationId,
    holder: &Holder,
    now: i64,
    expires: i64,
    budget: u32,
) -> Result<Vec<Lease>> {
    let slot: usize = db.query_row("SELECT scheduler_slot FROM sync_local_head WHERE singleton=1", [], |row| {
        row.get::<_, u8>(0).map(usize::from)
    })?;
    let mut quotas = [0u32; 4];
    for offset in 0..budget as usize {
        quotas[WHEEL[(slot + offset) % WHEEL.len()] as usize] += 1;
    }
    let mut remaining = budget;
    let mut leases = Vec::new();
    for (priority, quota) in quotas.into_iter().enumerate() {
        let candidates = load(db, priority as u8, now, quota)?;
        claim_rows(db, owner, holder, expires, candidates, &mut remaining, &mut leases)?;
    }
    // An empty class lends its quota; ready work need not wait for another call.
    for priority in 0..4 {
        if remaining == 0 {
            break;
        }
        let candidates = load(db, priority, now, remaining)?;
        claim_rows(db, owner, holder, expires, candidates, &mut remaining, &mut leases)?;
    }
    if remaining != budget {
        db.execute(
            "UPDATE sync_local_head SET scheduler_slot=?1 WHERE singleton=1",
            [((slot + budget as usize) % WHEEL.len()) as u32],
        )?;
    }
    leases.sort_by_key(|lease| number(lease.task.priority));
    Ok(leases)
}
fn load(db: &Connection, priority: u8, now: i64, limit: u32) -> Result<Vec<Row>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    for state in ["ready", "leased"] {
        let mut query=db.prepare_cached("SELECT kind,hub,scope,task_key,wanted,completed,state,priority,next_due,lease_generation,lease_owner,attempts,dependency,reason,holding,lease_wanted FROM sync_tasks WHERE state=?1 AND priority=?2 AND next_due<=?3 ORDER BY next_due,hub,scope,kind,task_key LIMIT ?4")?;
        candidates.extend(
            query.query_map(params![state, priority, now, limit], rows::read)?.collect::<rusqlite::Result<Vec<_>>>()?,
        );
    }
    candidates.sort_by(|a, b| {
        (a.task.next_due_millis, &a.task.key.scope, &a.task.key.kind, &a.task.key.key).cmp(&(
            b.task.next_due_millis,
            &b.task.key.scope,
            &b.task.key.kind,
            &b.task.key.key,
        ))
    });
    candidates.truncate(limit as usize);
    Ok(candidates)
}
fn claim_rows(
    db: &Connection,
    owner: &OperationId,
    holder: &Holder,
    expires: i64,
    candidates: Vec<Row>,
    remaining: &mut u32,
    leases: &mut Vec<Lease>,
) -> Result<()> {
    for mut row in candidates {
        *remaining -= 1;
        if !current_holding(db, holder, row.task.holding.as_ref())? {
            row.state = "superseded".into();
            row.owner = None;
            save(db, &row)?;
            continue;
        }
        row.generation = next(row.generation)?;
        row.owner = Some(owner.clone());
        row.lease_wanted = Some(row.task.wanted);
        row.state = "leased".into();
        row.task.next_due_millis = expires;
        row.attempts = row.attempts.saturating_add(1);
        save(db, &row)?;
        leases.push(Lease { task: row.task, generation: row.generation, owner: owner.clone() });
    }
    Ok(())
}
