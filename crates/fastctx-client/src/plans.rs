//! Read, edit and observe plans without contacting the conversation's host.

use crate::{Error, World, account_records::RecordsWatch};
use fastctx_types::{
    AccountName, Call, HubName, MachineName, Reply, ResidentId,
    account_records::{Metadata, Watched},
    plan::{self, Page},
};

impl World {
    pub async fn plan_read(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        owner: ResidentId,
        machine: Option<MachineName>,
    ) -> Result<Page, Error> {
        #[cfg(feature = "replica")]
        let copy = (hub.clone(), account.clone(), owner.clone(), machine.clone());
        let read = self.ask(Call::PlanRead { hub, account, owner, machine }).await;
        // The hub not reached — not a refusal — a device keeping a replica
        // reads the plan it holds there (§28.39).
        #[cfg(feature = "replica")]
        if let Err(error) = &read
            && !error.is_refusal()
            && let Some(device) = self.device()
            && let Some(page) =
                crate::device::copy_reads::plan(device, copy.0.as_ref(), copy.1.as_ref(), &copy.2, copy.3.as_ref())
        {
            return Ok(page);
        }
        match read? {
            Reply::Plan { result: plan::Result::Page { page } } => Ok(page),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
    pub async fn plan_write(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        owner: ResidentId,
        expected: Option<u64>,
        markdown: String,
        machine: Option<MachineName>,
    ) -> Result<plan::Result, Error> {
        match self.ask(Call::PlanWrite { hub, account, owner, expected, markdown, machine }).await? {
            Reply::Plan { result } => Ok(result),
            other => Err(Error::Unexpected(format!("{other:?}"))),
        }
    }
    pub async fn watch_plan(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        owner: ResidentId,
    ) -> Result<PlanWatch, Error> {
        Ok(PlanWatch {
            records: self.watch_records_for(hub, account, plan::KIND.into(), Some(owner.clone())).await?,
            owner,
        })
    }
}

pub struct PlanWatch {
    records: RecordsWatch,
    owner: ResidentId,
}
impl PlanWatch {
    pub async fn next(&mut self) -> Result<Option<Metadata>, Error> {
        loop {
            match self.records.next().await? {
                Watched::Current { entries } => {
                    return Ok(entries.into_iter().find(|entry| entry.key == self.owner.as_str()));
                }
                Watched::Changed { metadata } if metadata.key == self.owner.as_str() => return Ok(Some(metadata)),
                Watched::Changed { .. } => {}
            }
        }
    }
}
