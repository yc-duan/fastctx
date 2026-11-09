//! Dependency observations survive both lease races and process restarts.
use super::*;

pub(super) fn sequence(db: &Connection) -> Result<LocalSequence> {
    db.query_row("SELECT dependency_sequence FROM sync_local_head WHERE singleton=1", [], |r| sql::counter(r, 0))
        .map(LocalSequence)
        .map_err(Into::into)
}
pub(super) fn changed(db: &Connection, dependency: &str) -> Result<()> {
    let next = sequence(db)?.0.checked_add(1).ok_or_else(|| sql::invalid("dependency sequence exhausted"))?;
    db.execute("UPDATE sync_local_head SET dependency_sequence=?1 WHERE singleton=1", [next.to_be_bytes().as_slice()])?;
    db.execute("INSERT INTO sync_dependencies(dependency,sequence) VALUES(?1,?2) ON CONFLICT(dependency) DO UPDATE SET sequence=excluded.sequence",params![dependency,next.to_be_bytes().as_slice()])?;
    Ok(())
}
pub(super) fn since(db: &Connection, dependency: &str, claimed: LocalSequence) -> Result<bool> {
    let latest = db
        .query_row("SELECT sequence FROM sync_dependencies WHERE dependency=?1", [dependency], |r| sql::counter(r, 0))
        .optional()?
        .unwrap_or(0);
    Ok(latest > claimed.0)
}
pub(super) fn wake_page(db: &Connection, dependency: &str) -> Result<usize> {
    let mut query = db.prepare_cached(&format!(
        "SELECT {} FROM sync_tasks WHERE dependency=?1 AND state='blocked' ORDER BY next_due LIMIT 256",
        rows::COLUMNS
    ))?;
    let rows = query.query_map([dependency], rows::read)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let changed = rows.len();
    for mut row in rows {
        row.task.dependency = None;
        row.task.next_due_millis = 0;
        row.blocked_attempts = 0;
        row.state = if row.task.completed < row.task.wanted { "ready" } else { "complete" }.into();
        row.reason = None;
        save(db, &row)?;
    }
    Ok(changed)
}
