//! Plan access names the conversation, not an account to impersonate.

use super::Link;
use fastctx_types::{
    AccountName, Address,
    account_records::{Request as Records, Response as Recorded},
    owned_records::{Request, Response},
    plan::{self, Author, Body, Page},
};

impl Link {
    pub async fn owned_plan_read(&self, account: &AccountName, owner: &Address) -> Result<Page, String> {
        let response = self
            .owned_records(
                account,
                Request::Record {
                    conversation: owner.clone(),
                    request: Records::Read { kind: plan::KIND.into(), key: owner.resident.to_string() },
                },
            )
            .await?;
        let Response::Record { response: Recorded::Read { record } } = response else {
            return Err(plan::words::REPLY.into());
        };
        let (metadata, body) = match record {
            None => (None, None),
            Some(record) => {
                let body = if record.metadata.deleted {
                    None
                } else {
                    Some(serde_json::from_value::<Body>(record.body).map_err(plan::words::invalid_body)?)
                };
                (Some(record.metadata), body)
            }
        };
        let page = Page { owner: owner.resident.clone(), metadata, body };
        if page.body.as_ref().and_then(Body::of).is_some_and(|address| address.resident != owner.resident) {
            return Err("the plan record belongs to another conversation".into());
        }
        Ok(page)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn owned_plan_write(
        &self,
        account: &AccountName,
        owner: &Address,
        expected: Option<u64>,
        markdown: String,
        author: Author,
        submission: Option<String>,
    ) -> Result<plan::Result, String> {
        plan::validate(&markdown)?;
        let body =
            Body { markdown, author, owner: Some(Box::new(owner.clone())), submission: submission.map(Box::new) };
        let response = self
            .owned_records(
                account,
                Request::Record {
                    conversation: owner.clone(),
                    request: Records::Write {
                        kind: plan::KIND.into(),
                        key: owner.resident.to_string(),
                        expected,
                        body: serde_json::to_value(&body).map_err(|error| error.to_string())?,
                    },
                },
            )
            .await?;
        match response {
            Response::Record { response: Recorded::Written { metadata } } => Ok(plan::Result::Page {
                page: Page { owner: owner.resident.clone(), metadata: Some(metadata), body: Some(body) },
            }),
            Response::Record { response: Recorded::Conflict { current } } => Ok(plan::Result::Conflict { current }),
            _ => Err(plan::words::REPLY.into()),
        }
    }
}
