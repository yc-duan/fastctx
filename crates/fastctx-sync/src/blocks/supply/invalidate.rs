//! Revocation coordinators resume bounded invalidation by a stable check key.
use super::*;
use crate::identity::{Digest, ObjectId, OperationId, ScopeKey};
use fastctx_storage::rusqlite::OptionalExtension;

pub(crate) struct InvalidationPage {
    pub changed: u32,
    pub next: Option<SupplyTicket>,
}
impl SupplyWriter<'_> {
    pub(crate) fn invalidate(
        &self,
        scope: &ScopeKey,
        object: Option<&ObjectId>,
        operation: Option<&OperationId>,
        after: Option<&SupplyTicket>,
        max: u32,
    ) -> Result<InvalidationPage> {
        if max == 0 || max > 64 || after.is_some_and(|cursor| &cursor.scope != scope) {
            return Err(sql::invalid("supply invalidation needs a bounded page in the same scope"));
        }
        let filter = match (object, operation) {
            (Some(_), Some(_)) => " AND object=?3 AND operation=?4",
            (Some(_), None) => " AND object=?3",
            (None, Some(_)) => " AND operation=?4",
            (None, None) => "",
        };
        let statement = format!(
            "SELECT id,generation FROM sync_supply_checks WHERE hub=?1 AND scope=?2{filter} AND phase IN ('walking','waiting','ready') AND (id,generation)>(?5,?6) ORDER BY id,generation LIMIT ?7"
        );
        let selected = self
            .db
            .prepare_cached(&statement)?
            .query_map(
                params![
                    scope.hub.to_string(),
                    scope.scope.as_str(),
                    object.map(ObjectId::as_str),
                    operation.map(OperationId::as_str),
                    after.map_or("", |cursor| cursor.id.as_str()),
                    after.map_or(0, |cursor| cursor.generation.0).to_be_bytes().as_slice(),
                    max
                ],
                |row| Ok((row.get::<_, String>(0)?, sql::counter(row, 1)?)),
            )?
            .collect::<fastctx_storage::rusqlite::Result<Vec<_>>>()?;
        let full = selected.len() == max as usize;
        let mut changed = 0;
        let mut last = None;
        for (id, generation) in selected {
            let ticket = SupplyTicket {
                scope: scope.clone(),
                id: Digest::try_from(id).map_err(sql::invalid)?,
                generation: Revision(generation),
            };
            changed += u32::from(self.invalidate_one(&ticket)?);
            last = Some(ticket);
        }
        Ok(InvalidationPage { changed, next: if full { last } else { None } })
    }
    fn invalidate_one(&self, ticket: &SupplyTicket) -> Result<bool> {
        let mut check = self.owned(ticket)?;
        let scope = &ticket.scope;
        let deleted: bool = self.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_tombstones WHERE hub=?1 AND scope=?2 AND object=?3)",
            params![scope.hub.to_string(), scope.scope.as_str(), check.request.object.object.as_str()],
            |row| row.get(0),
        )?;
        if deleted {
            self.cancel(ticket, CancelReason::Deleted)?;
            return Ok(true);
        }
        let state = self
            .db
            .query_row(
                "SELECT enabled,acl_revision,publication_epoch FROM sync_scopes WHERE hub=?1 AND scope=?2",
                params![scope.hub.to_string(), scope.scope.as_str()],
                |row| Ok((row.get::<_, bool>(0)?, sql::counter(row, 1)?, sql::counter(row, 2)?)),
            )
            .optional()?;
        if state.is_none_or(|(enabled, _, _)| !enabled) {
            self.cancel(ticket, CancelReason::Revoked)?;
            return Ok(true);
        }
        let _boundary = boundary::enter_typed_write(Role::Supply);
        let expected = &check.request.holding;
        let reason = if state != Some((true, expected.acl_revision.0, expected.publication_epoch.0)) {
            PauseReason::HoldingChanged
        } else {
            match qualification::current(self.db, &check)? {
                None => {
                    self.cancel(ticket, CancelReason::UploadCancelled)?;
                    return Ok(true);
                }
                Some(revision) if revision != check.supply_revision => PauseReason::SupplyChanged,
                _ => return Ok(false),
            }
        };
        rows::progress(self.db, &mut check, "paused", SupplyProgress::Paused { reason })?;
        cleanup::enqueue(self.db, &check)?;
        Ok(true)
    }
}

/// Closed uploads have no valid checks. Each task removes one active page, so
/// the next page starts from the beginning without skipping any live work.
pub(crate) fn cancelled_upload(
    db: &Connection,
    scope: &ScopeKey,
    operation: &OperationId,
    object: &ObjectId,
) -> Result<()> {
    let pending:bool=db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_supply_checks WHERE hub=?1 AND scope=?2 AND operation=?3 AND object=?4 AND phase IN ('walking','waiting','ready'))",
        params![scope.hub.to_string(),scope.scope.as_str(),operation.as_str(),object.as_str()],|row|row.get(0),
    )?;
    if pending {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        crate::scheduler::supply_cleanup(
            db,
            scope,
            format!("supply-cancel:{}:{}", operation.as_str(), object.as_str()),
        )?;
    }
    Ok(())
}
