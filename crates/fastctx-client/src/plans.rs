//! Read, edit and observe plans without contacting the conversation's host.

use crate::{Error, World, account_records::RecordsWatch};
use fastctx_types::{
    AccountName, Call, HubName, Reply, ResidentId,
    account_records::{Metadata, Watched},
    plan::{self, Page},
};

impl World {
    pub async fn plan_read(
        &self,
        hub: Option<HubName>,
        account: Option<AccountName>,
        owner: ResidentId,
    ) -> Result<Page, Error> {
        match self.ask(Call::PlanRead { hub, account, owner }).await? {
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
    ) -> Result<plan::Result, Error> {
        match self.ask(Call::PlanWrite { hub, account, owner, expected, markdown }).await? {
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
        Ok(PlanWatch { records: self.watch_account_records(hub, account, plan::KIND.into()).await?, owner })
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
