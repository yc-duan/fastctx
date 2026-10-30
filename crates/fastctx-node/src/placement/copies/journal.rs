//! A copy journals submission and file publication separately. An accepted
//! submission is replayed by identity; a file published before a crash advances
//! its baseline without sending the same content as a fresh edit.

use super::*;
use fastctx_types::plan::Page;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Submission {
    pub id: String,
    pub base: Option<u64>,
    pub text: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Publication {
    pub before: Option<String>,
    pub text: String,
    pub page: Page,
}

pub(super) async fn settle(
    node: &Arc<Node>,
    slot: &Slot,
    state: &mut State,
    link: &fastctx_net::link::Link,
) -> Result<(), Failure> {
    if let Some(submission) = state.submission.clone() {
        let page = match link
            .plan_copy(
                &state.account.account,
                Request::Merge {
                    owner: state.owner.clone(),
                    base: submission.base,
                    markdown: submission.text.clone(),
                    submission: Some(submission.id),
                },
            )
            .await
            .map_err(failure)?
        {
            Reply::Page { page } => *page,
            _ => return Err(failure("the hub did not return the accepted plan submission")),
        };
        let current = read(&slot.path)?;
        let accepted = page.body.as_ref().map_or("", |body| body.markdown.as_str());
        let text = if current.as_deref().unwrap_or_default() == submission.text || current.as_deref() == Some(accepted)
        {
            accepted.to_string()
        } else {
            fastctx_net::plan_merge::merge(&submission.text, accepted, current.as_deref().unwrap_or_default())
        };
        state.publication = Some(Publication { before: current, text, page });
        state.submission = None;
        keep(node, &slot.key, state)?;
    }
    if let Some(mut publication) = state.publication.clone() {
        let current = read(&slot.path)?;
        if current != publication.before && current.as_deref() != Some(&publication.text) {
            publication.text = fastctx_net::plan_merge::merge(
                publication.before.as_deref().unwrap_or_default(),
                &publication.text,
                current.as_deref().unwrap_or_default(),
            );
            publication.before = current;
            state.publication = Some(publication.clone());
            keep(node, &slot.key, state)?;
        }
        if read(&slot.path)?.as_deref() != Some(&publication.text) {
            fastctx_platform::publish_checked(
                &slot.path,
                publication.text.as_bytes(),
                fastctx_platform::Permissions::Private,
                true,
                || {
                    if read(&slot.path).map_err(|error| std::io::Error::other(error.message))? != publication.before {
                        return Err(std::io::Error::other("the plan file changed before publication"));
                    }
                    Ok(())
                },
            )
            .map_err(|error| failure(error.error))?;
        }
        state.base = publication.page.version();
        state.text = publication.page.body.map_or_else(String::new, |body| body.markdown);
        state.initial = false;
        state.publication = None;
        keep(node, &slot.key, state)?;
    }
    Ok(())
}
