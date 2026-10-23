//! Typed plans use account records, so the conversation's host need not be online.

use super::Link;
use fastctx_types::{
    AccountName, ResidentId,
    account_records::{Request, Response},
    plan::{self, Author, Body, Page, words},
};

impl Link {
    pub async fn plan_read(&self, account: &AccountName, owner: ResidentId) -> Result<Page, String> {
        if !self.view().conversation_plans {
            return Err(words::OLDER.into());
        }
        let response =
            self.account_records(account, Request::Read { kind: plan::KIND.into(), key: owner.to_string() }).await?;
        let Response::Read { record } = response else { return Err(words::REPLY.into()) };
        let (metadata, body) = match record {
            None => (None, None),
            Some(record) => {
                let body = if record.metadata.deleted {
                    None
                } else {
                    Some(serde_json::from_value(record.body).map_err(words::invalid_body)?)
                };
                (Some(record.metadata), body)
            }
        };
        Ok(Page { owner, metadata, body })
    }

    pub async fn plan_write(
        &self,
        account: &AccountName,
        owner: ResidentId,
        expected: Option<u64>,
        markdown: String,
        author: Author,
    ) -> Result<plan::Result, String> {
        if !self.view().conversation_plans {
            return Err(words::OLDER.into());
        }
        plan::validate(&markdown)?;
        let body = Body { markdown, author };
        match self
            .account_records(
                account,
                Request::Write {
                    kind: plan::KIND.into(),
                    key: owner.to_string(),
                    expected,
                    body: serde_json::to_value(&body).map_err(|error| error.to_string())?,
                },
            )
            .await?
        {
            Response::Written { metadata } => {
                Ok(plan::Result::Page { page: Page { owner, metadata: Some(metadata), body: Some(body) } })
            }
            Response::Conflict { current } => Ok(plan::Result::Conflict { current }),
            _ => Err(words::REPLY.into()),
        }
    }

    /// Records absence even when no page was written, fencing off a concurrent
    /// first save. A conflict leaves deletion pending for its next attempt.
    pub async fn plan_delete(&self, account: &AccountName, owner: &ResidentId) -> Result<(), String> {
        let page = self.plan_read(account, owner.clone()).await?;
        if page.deleted() {
            return Ok(());
        }
        match self
            .account_records(
                account,
                Request::Delete { kind: plan::KIND.into(), key: owner.to_string(), expected: page.version() },
            )
            .await?
        {
            Response::Written { .. } => Ok(()),
            Response::Conflict { .. } => Err(words::CONFLICT.into()),
            _ => Err(words::REPLY.into()),
        }
    }
}
