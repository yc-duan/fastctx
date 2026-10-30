//! Typed plans use account records, so the conversation's host need not be online.

use super::Link;
use fastctx_types::{
    AccountName, Address, ResidentId,
    account_records::{Request, Response},
    plan::{self, Author, Body, Page, words},
};

impl Link {
    /// The conversation other than `whose` that `page` is the plan of: one
    /// on another machine with the same short name (`plan::Body::of`).
    pub fn plan_of_another<'a>(&self, page: &'a Page, whose: &Address) -> Option<&'a Address> {
        let view = self.view();
        page.body
            .as_ref()
            .and_then(|body| body.of())
            .filter(|of| of.resident != whose.resident || !view.same(&of.machine, &whose.machine))
    }

    /// The plan of the conversation `whose`, as `plan_read`: another's of the
    /// same short name reads as none.
    pub async fn plan_read_of(&self, account: &AccountName, whose: &Address) -> Result<Page, String> {
        let page = self.plan_read(account, whose.resident.clone()).await?;
        Ok(if self.plan_of_another(&page, whose).is_some() {
            Page { owner: page.owner, metadata: None, body: None }
        } else {
            page
        })
    }

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

    /// Writes the plan of `owner`. The conversation writes its own; the user's
    /// edit keeps whose it is, or, of one that does not say, makes it
    /// `whose` when the user names the conversation, and is refused where
    /// it is another's of the same short name.
    pub async fn plan_write(
        &self,
        account: &AccountName,
        owner: ResidentId,
        expected: Option<u64>,
        markdown: String,
        author: Author,
        whose: Option<&Address>,
    ) -> Result<plan::Result, String> {
        if !self.view().conversation_plans {
            return Err(words::OLDER.into());
        }
        plan::validate(&markdown)?;
        let owned = match &author {
            Author::Resident { address } => Some(Box::new(address.clone())),
            Author::User => {
                let current = self.plan_read(account, owner.clone()).await?;
                if let Some(whose) = whose
                    && let Some(other) = self.plan_of_another(&current, whose)
                {
                    return Err(words::elsewhere(other));
                }
                current.body.and_then(|body| body.owner).or_else(|| whose.cloned().map(Box::new))
            }
        };
        let body = Body { markdown, author, owner: owned, submission: None };
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
