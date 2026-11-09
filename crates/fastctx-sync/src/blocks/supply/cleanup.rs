//! Cancellation closes adoption before bounded retirement of private holds and work.
use super::*;
use fastctx_storage::rusqlite::OptionalExtension;

#[derive(Clone, Copy, Debug)]
pub(crate) struct CleanupProgress {
    pub released: u32,
    pub done: bool,
}
impl SupplyWriter<'_> {
    pub(crate) fn cancel(&self, ticket: &SupplyTicket, reason: CancelReason) -> Result<()> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        let mut check = self.owned(ticket)?;
        if check.phase == "consumed" || check.phase == "cancelled" {
            return Ok(());
        }
        rows::progress(self.db, &mut check, "cancelled", SupplyProgress::Cancelled)?;
        self.db.execute(
            "UPDATE sync_supply_checks SET cancel_reason=?1 WHERE hub=?2 AND scope=?3 AND id=?4 AND generation=?5",
            params![
                model::encode(&reason)?,
                ticket.scope.hub.to_string(),
                ticket.scope.scope.as_str(),
                ticket.id.as_str(),
                ticket.generation.0.to_be_bytes().as_slice()
            ],
        )?;
        enqueue(self.db, &check)
    }
    pub(crate) fn cleanup(&self, ticket: &SupplyTicket, max: u32) -> Result<CleanupProgress> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        if max == 0 || max > 64 {
            return Err(sql::invalid("supply cleanup needs one to 64 entries"));
        }
        let check = self.owned(ticket)?;
        if matches!(check.phase.as_str(), "walking" | "waiting" | "ready") {
            return Err(sql::invalid("active supply work cannot be cleaned"));
        }
        let released = protection::retire_page(self.db, ticket, max)?;
        let remaining = max - released;
        let scope = &ticket.scope;
        if remaining != 0 {
            self.db.execute(
                "DELETE FROM sync_supply_work WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4 AND position IN (SELECT position FROM sync_supply_work WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4 ORDER BY position LIMIT ?5)",
                params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice(),remaining],
            )?;
        }
        let pending:bool=self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_supply_work WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4) OR EXISTS(SELECT 1 FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND supply_id=?3 AND supply_generation=?4 AND released=0)",
            params![scope.hub.to_string(),scope.scope.as_str(),ticket.id.as_str(),ticket.generation.0.to_be_bytes().as_slice()],|row|row.get(0),
        )?;
        if !pending {
            self.db.execute(
                "UPDATE sync_supply_checks SET cleanup=0 WHERE hub=?1 AND scope=?2 AND id=?3 AND generation=?4",
                params![
                    scope.hub.to_string(),
                    scope.scope.as_str(),
                    ticket.id.as_str(),
                    ticket.generation.0.to_be_bytes().as_slice()
                ],
            )?;
        }
        let progress = CleanupProgress { released, done: !pending };
        use crate::metrics::{Cause, Counter, Metrics, PlatformMetrics};
        PlatformMetrics.add(Counter::SupplyRetiredHolds, Cause::Ordinary, u64::from(progress.released));
        Ok(progress)
    }
}

pub(super) fn enqueue(db: &Connection, check: &rows::Check) -> Result<()> {
    crate::scheduler::supply_cleanup(
        db,
        &check.ticket.scope,
        format!("supply:{}:{}", check.ticket.id.as_str(), check.ticket.generation.0),
    )
}

/// Scope retirement must first invalidate the check owning this private lease.
pub(crate) fn invalidate_hold(
    db: &Connection,
    scope: &crate::identity::ScopeKey,
    lease: &crate::identity::RootLeaseId,
) -> Result<()> {
    let row = db
        .query_row(
            "SELECT supply_id,supply_generation FROM sync_root_leases WHERE hub=?1 AND scope=?2 AND lease=?3",
            params![scope.hub.to_string(), scope.scope.as_str(), lease.as_str()],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
        )
        .optional()?;
    let Some((Some(id), Some(generation))) = row else {
        return Ok(());
    };
    let generation: [u8; 8] =
        generation.try_into().map_err(|_| sql::invalid("invalid supply protection generation"))?;
    let ticket = SupplyTicket {
        scope: scope.clone(),
        id: crate::identity::Digest::try_from(id).map_err(sql::invalid)?,
        generation: Revision(u64::from_be_bytes(generation)),
    };
    let Some(mut check) = rows::get(db, &ticket)? else {
        return Err(sql::invalid("supply protection has no owner"));
    };
    if matches!(check.phase.as_str(), "walking" | "waiting" | "ready") {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        rows::progress(db, &mut check, "cancelled", SupplyProgress::Cancelled)?;
        enqueue(db, &check)?;
    }
    Ok(())
}
