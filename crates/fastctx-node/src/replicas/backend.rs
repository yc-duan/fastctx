//! Revision reads and idempotent submissions for both replica kinds.

use super::state::{Copy, Pending};
use fastctx_net::link::Link;
use fastctx_types::{
    owned_records::{Request, Response},
    replicas::{Document, Head, Scope},
};

pub(crate) async fn accepted(link: &Link, copy: &Copy, pending: &Pending) -> Result<bool, String> {
    let (owner, kind, key) = match &copy.head.document {
        Document::History { scope: Scope::Project { town, .. }, name } => {
            return link.history_submitted(&copy.account.account, town, name, &pending.id).await;
        }
        Document::Plan { owner } => (owner, fastctx_types::plan::KIND, owner.resident.to_string()),
        Document::History { scope: Scope::Conversation { conversation }, name } => (
            &conversation.address,
            fastctx_types::history::RECORD_KIND,
            format!("{}/{name}", conversation.address.resident),
        ),
    };
    match link
        .owned_records(
            &copy.account.account,
            Request::Submitted { conversation: owner.clone(), kind: kind.into(), key, submission: pending.id.clone() },
        )
        .await?
    {
        Response::Submitted { record } => Ok(record.is_some()),
        _ => Err("the hub did not return the replica submission receipt".into()),
    }
}

pub(crate) async fn base(link: &Link, copy: &Copy, version: Option<u64>) -> Result<String, String> {
    let Some(version) = version else { return Ok(String::new()) };
    match &copy.head.document {
        Document::Plan { owner } => {
            let response = link
                .owned_records(
                    &copy.account.account,
                    Request::Revision {
                        conversation: owner.clone(),
                        kind: fastctx_types::plan::KIND.into(),
                        key: owner.resident.to_string(),
                        version,
                    },
                )
                .await?;
            let Response::Revision { record } = response else {
                return Err("the base plan revision was not returned".into());
            };
            let body: fastctx_types::plan::Body =
                serde_json::from_value(record.body).map_err(|error| error.to_string())?;
            Ok(body.markdown)
        }
        Document::History { scope: Scope::Project { town, .. }, name } => {
            Ok(link.history_read(&copy.account.account, town, name, Some(version)).await?.text)
        }
        Document::History { scope: Scope::Conversation { conversation }, name } => link
            .conversation_history_stored(&copy.account.account, &conversation.address, name, Some(version))
            .await?
            .map(|stored| stored.text)
            .ok_or_else(|| "the base history revision is unavailable".into()),
    }
}

pub(crate) struct Submitted {
    pub head: Head,
    pub text: String,
}

/// The accepted revision, not yet installed locally; none on a CAS collision.
/// An unknown outcome remains an error with its receipt pending.
pub(crate) async fn submit(
    link: &Link,
    copy: &Copy,
    pending: &Pending,
    text: String,
) -> Result<Option<Submitted>, String> {
    match &copy.head.document {
        Document::Plan { owner } => {
            let author = match &pending.author.stamp.by {
                fastctx_types::project::Writer::Resident { address } => {
                    fastctx_types::plan::Author::Resident { address: address.clone() }
                }
                _ => fastctx_types::plan::Author::User,
            };
            let result = link
                .owned_plan_write(
                    &copy.account.account,
                    owner,
                    copy.head.version,
                    text,
                    author,
                    Some(pending.id.clone()),
                )
                .await?;
            match result {
                fastctx_types::plan::Result::Conflict { .. } => Ok(None),
                fastctx_types::plan::Result::Page { page } => {
                    let mut head = copy.head.clone();
                    head.version = page.version();
                    head.retired = page.deleted();
                    Ok(Some(Submitted { head, text: page.body.map_or(String::new(), |body| body.markdown) }))
                }
            }
        }
        Document::History { scope, name } => {
            let result = match scope {
                Scope::Project { town, .. } => {
                    let (revision, files) = fastctx_net::topic_history::revision_files(
                        name,
                        &text,
                        copy.head.version,
                        copy.head.retired,
                        pending.author.clone(),
                        Some(pending.id.clone()),
                    )?;
                    let entry = link.sign_entry(&link.entry(town, None, files));
                    link.history_write(&copy.account.account, town, entry).await.map(|_| revision)
                }
                Scope::Conversation { conversation } => {
                    link.conversation_history_save(
                        &copy.account.account,
                        &conversation.address,
                        name,
                        text.clone(),
                        copy.head.version,
                        copy.head.retired,
                        pending.author.clone(),
                        Some(pending.id.clone()),
                    )
                    .await
                }
            };
            let revision = match result {
                Ok(revision) => revision,
                Err(error) if error.starts_with("the document changed;") => return Ok(None),
                Err(error) => return Err(error),
            };
            let mut head = copy.head.clone();
            head.version = Some(revision.version);
            head.retired = revision.retired;
            head.history = Some(revision);
            Ok(Some(Submitted { head, text }))
        }
    }
}
