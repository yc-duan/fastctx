//! Durable supply verification adopts content only inside the owning write transaction.
pub(crate) mod audit;
pub(crate) use cleanup::invalidate_hold;
pub(crate) use invalidate::cancelled_upload;
mod cleanup;
mod invalidate;
mod layout;
mod model;
mod pin_audit;
mod protection;
mod qualification;
mod rows;
mod timeline;
mod walk;

use crate::{
    blocks::{access, pins, root_holds, rows as content},
    cursor::{Holder, Revision},
    store::{
        access::{AccessPolicy, Custody},
        boundary::{self, Role},
        roots::{AcquireRoot, RootLease, RootUse},
        sql,
    },
};
use fastctx_storage::{
    Result,
    rusqlite::{Connection, params},
};
pub(crate) use model::{CancelReason, SupplyBudget, SupplyOwner, SupplyProgress, SupplyRequest, SupplyTicket};
use model::{PauseReason, Request};
use std::{marker::PhantomData, rc::Rc};

pub(crate) struct SupplyWriter<'tx> {
    db: &'tx Connection,
    holder: &'tx Holder,
    policy: &'tx dyn AccessPolicy,
    now: i64,
}
pub(crate) struct ReadySupply<'tx> {
    db: &'tx Connection,
    check: rows::Check,
    marker: PhantomData<Rc<()>>,
}
pub(crate) fn writer<'tx>(
    db: &'tx Connection,
    holder: &'tx Holder,
    policy: &'tx dyn AccessPolicy,
    now: i64,
) -> Result<SupplyWriter<'tx>> {
    if db.is_autocommit() {
        return Err(sql::invalid("supply verification needs the owning write transaction"));
    }
    Ok(SupplyWriter { db, holder, policy, now })
}
impl<'tx> SupplyWriter<'tx> {
    pub(crate) fn begin(&self, request: &SupplyRequest) -> Result<SupplyTicket> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        pins::authorize(self.db, self.holder, self.policy, &request.custody)?;
        let request = Request::capture(request)?;
        let revision = qualification::open(self.db, &request)?;
        let mut previous = rows::latest(self.db, &request.object.scope, request.identity()?)?;
        let generation = if let Some(previous) = previous.as_mut() {
            if previous.phase == "consumed" && audit::retained(self.db, &previous.request)? {
                return Ok(previous.ticket.clone());
            }
            if matches!(previous.phase.as_str(), "cancelled" | "consumed") {
                return Err(sql::invalid("a terminal supply request cannot be adopted again"));
            }
            if previous.supply_revision == revision
                && matches!(previous.phase.as_str(), "walking" | "waiting" | "ready")
            {
                return Ok(previous.ticket.clone());
            }
            rows::progress(self.db, previous, "paused", SupplyProgress::Paused { reason: PauseReason::SupplyChanged })?;
            cleanup::enqueue(self.db, previous)?;
            Revision(
                previous
                    .ticket
                    .generation
                    .0
                    .checked_add(1)
                    .ok_or_else(|| sql::invalid("supply generation overflow"))?,
            )
        } else {
            Revision(1)
        };
        let check = rows::create(self.db, request, generation, revision)?;
        protection::bases(self.db, &check, previous.as_ref(), self.now)?;
        self.dirty(&check)?;
        Ok(check.ticket)
    }
    pub(crate) fn step(&self, ticket: &SupplyTicket, budget: SupplyBudget) -> Result<SupplyProgress> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        let mut check = self.owned(ticket)?;
        if !matches!(check.phase.as_str(), "walking" | "waiting" | "ready") {
            return Ok(check.progress);
        }
        let custody = check.request.custody()?;
        if let Err(error) = pins::authorize(self.db, self.holder, self.policy, &custody) {
            if error.kind() != fastctx_storage::ErrorKind::Invalid {
                return Err(error);
            }
            let progress = SupplyProgress::Paused { reason: PauseReason::HoldingChanged };
            rows::progress(self.db, &mut check, "paused", progress.clone())?;
            cleanup::enqueue(self.db, &check)?;
            return Ok(progress);
        }
        match qualification::current(self.db, &check)? {
            None => {
                rows::progress(self.db, &mut check, "cancelled", SupplyProgress::Cancelled)?;
                cleanup::enqueue(self.db, &check)?;
                return Ok(SupplyProgress::Cancelled);
            }
            Some(revision) if revision != check.supply_revision => {
                let progress = SupplyProgress::Paused { reason: PauseReason::SupplyChanged };
                rows::progress(self.db, &mut check, "paused", progress.clone())?;
                cleanup::enqueue(self.db, &check)?;
                return Ok(progress);
            }
            Some(_) => {}
        }
        let progress = walk::step(self.db, &mut check, budget)?;
        if matches!(progress, SupplyProgress::Rejected { .. }) {
            cleanup::enqueue(self.db, &check)?;
        }
        Ok(progress)
    }
    pub(crate) fn recheck(&self, custody: &Custody, ticket: &SupplyTicket) -> Result<ReadySupply<'tx>> {
        let check = self.owned(ticket)?;
        let expected = check.request.custody()?;
        if custody.object != expected.object
            || custody.kind.registration().id != expected.kind.registration().id
            || custody.holding != expected.holding
            || custody.purpose != expected.purpose
        {
            return Err(sql::invalid("supply receipt belongs to another object custody"));
        }
        pins::authorize(self.db, self.holder, self.policy, custody)?;
        let current_supply = qualification::current(self.db, &check)?;
        let qualified = match check.phase.as_str() {
            "ready" => current_supply == Some(check.supply_revision),
            "consumed" => current_supply.is_some() && audit::retained(self.db, &check.request)?,
            _ => false,
        };
        if !qualified || !content::complete(self.db, &check.request.root)? {
            return Err(fastctx_storage::Error::with_kind(
                fastctx_storage::ErrorKind::Busy,
                "supply validation is incomplete or has changed",
            ));
        }
        Ok(ReadySupply { db: self.db, check, marker: PhantomData })
    }
    pub(crate) fn acquire(&self, ready: ReadySupply<'tx>, request: &AcquireRoot) -> Result<RootLease> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        self.same_transaction(&ready)?;
        request.validate().map_err(|e| sql::invalid(e.to_string()))?;
        if request.expires_at_millis.is_some_and(|end| end <= self.now) {
            return Err(sql::invalid("root usage has already expired"));
        }
        let expected = &ready.check.request;
        let usage = match &expected.owner {
            SupplyOwner::Use { usage } => usage == &request.usage,
            SupplyOwner::Source { .. } | SupplyOwner::Mutation | SupplyOwner::Stream { .. } => false,
        };
        if !usage
            || expected.root != request.root
            || expected.object != request.custody.object
            || expected.kind != request.custody.kind.registration().id
            || expected.holding != request.custody.holding
            || expected.purpose != request.custody.purpose
        {
            return Err(sql::invalid("root usage differs from its completed supply check"));
        }
        pins::authorize(self.db, self.holder, self.policy, &request.custody)?;
        let lease = root_holds::insert(self.db, &expected.operation, request)?;
        self.consume(ready)?;
        Ok(lease)
    }
    pub(crate) fn retain_supplied(&self, ready: ReadySupply<'tx>) -> Result<()> {
        let _boundary = boundary::enter_typed_write(Role::Supply);
        self.same_transaction(&ready)?;
        let request = &ready.check.request;
        if matches!(request.owner, SupplyOwner::Use { .. }) {
            return Err(sql::invalid("a root-use check cannot grant asset reads"));
        }
        access::grant_root(self.db, &request.object, &request.kind, &request.root, &request.holding)?;
        self.consume(ready)
    }
    fn consume(&self, ready: ReadySupply<'tx>) -> Result<()> {
        let mut check = ready.check;
        rows::progress(self.db, &mut check, "consumed", SupplyProgress::Consumed)?;
        cleanup::enqueue(self.db, &check)?;
        self.cleanup(&check.ticket, 64)?;
        Ok(())
    }
    fn owned(&self, ticket: &SupplyTicket) -> Result<rows::Check> {
        let check = rows::get(self.db, ticket)?.ok_or_else(|| sql::invalid("supply work no longer exists"))?;
        if &check.request.holding.holder != self.holder {
            return Err(sql::invalid("supply work belongs to another holder"));
        }
        Ok(check)
    }
    fn same_transaction(&self, ready: &ReadySupply<'tx>) -> Result<()> {
        if !std::ptr::eq(self.db, ready.db) || self.db.is_autocommit() {
            return Err(sql::invalid("supply adoption left its verified transaction"));
        }
        self.recheck(&ready.check.request.custody()?, &ready.check.ticket)?;
        Ok(())
    }
    fn dirty(&self, check: &rows::Check) -> Result<()> {
        crate::scheduler::content_changed(
            self.db,
            &check.request.holding,
            format!("supply:{}:{}", check.ticket.id.as_str(), check.ticket.generation.0),
        )
    }
}
