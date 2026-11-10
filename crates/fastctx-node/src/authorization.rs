//! Resolves a conversation's subject and applies the directory policy before machine work.

use std::sync::Arc;

use fastctx_net::authorization::Action;
use fastctx_net::directory::View;
use fastctx_net::link::{Link, Status};
use fastctx_types::ResidentId;
use fastctx_types::access::{Acting, Purpose, Written};
use fastctx_types::authorization::{Denial, DenialReason};
use fastctx_types::ipc::Failure;

use crate::Node;

/// A known permission refusal is not an uncertain delivery and must not be retried as one.
pub(crate) fn call_failure(failure: fastctx_net::link::CallFailure) -> Failure {
    match failure {
        fastctx_net::link::CallFailure::NotAllowed { .. } | fastctx_net::link::CallFailure::Refused { .. } => {
            Failure::refused(failure.to_string())
        }
        _ => Failure::unanswered(failure.to_string()),
    }
}

pub(crate) fn no_subject() -> Failure {
    Denial::new(DenialReason::NoSubject, "this request has no recognized account identity").into()
}

/// An account action waits while its hub cannot provide current authority:
/// while the line is down, and on a new connection until the hub gave its
/// view on it, as what an earlier one left may lack a machine come since.
pub(crate) fn current_view(link: &Link) -> Result<Arc<View>, Failure> {
    let view = link.view();
    if !matches!(link.status(), Status::Online { .. }) || !link.view_current() || view.me().is_none() {
        return Err(Denial::new(
            DenialReason::ViewUnavailable,
            "the hub's current account view is unavailable; the action waits for it",
        )
        .into());
    }
    Ok(view)
}

/// The hub's current view on `link`, waited for `within` at most: for work
/// accepted earlier that begins as the node starts, before its hubs gave
/// their views, so that it is not judged by a view that was only late.
/// What is asked and answered at once does not wait; its caller asks again.
#[expect(dead_code, reason = "the submitted families that judge by a hub's view as their work begins take it up")]
pub(crate) async fn awaited_view(node: &Node, link: &Link, within: std::time::Duration) -> Result<Arc<View>, Failure> {
    let mut views = node.views.subscribe();
    let until = tokio::time::Instant::now() + within;
    loop {
        match current_view(link) {
            Err(pending) if view_pending(&pending) => {
                let now = tokio::time::Instant::now();
                if now >= until {
                    return Err(pending);
                }
                // A line that came up changes no view by itself: it is
                // looked at again each second as well.
                let next = (now + std::time::Duration::from_secs(1)).min(until);
                if let Ok(Err(_)) = tokio::time::timeout_at(next, views.changed()).await {
                    return Err(pending);
                }
            }
            judged => return judged,
        }
    }
}

/// Whether `failure` only says a hub's view of this connection has not come
/// yet: no judgement, which one made once it comes may reverse.
pub(crate) fn view_pending(failure: &Failure) -> bool {
    failure.denial == Some(DenialReason::ViewUnavailable)
}

pub(crate) fn resident(node: &Node, id: &ResidentId) -> Result<Acting, Failure> {
    let acting = crate::here::acting(node, id).ok_or_else(no_subject)?;
    if matches!(&acting, Acting::Accounts { accounts } if accounts.is_empty()) {
        return Err(no_subject());
    }
    Ok(acting)
}

impl Node {
    /// Local users keep their own OS access; explicit account sessions use the same policy as remote calls.
    pub(crate) fn authorize_here(&self, acting: &Acting) -> Result<(), Failure> {
        let Acting::Accounts { accounts } = acting else { return Ok(()) };
        if accounts.is_empty() {
            return Err(no_subject());
        }
        let mut refused = None;
        let mut pending = None;
        for account in accounts {
            let allowed = (|| {
                let link = self.link_of(&account.hub).ok_or_else(|| {
                    Failure::from(Denial::new(
                        DenialReason::ViewUnavailable,
                        "the account's hub is no longer connected",
                    ))
                })?;
                let view = match current_view(&link) {
                    Ok(view) => view,
                    Err(error)
                        if view_pending(&error)
                            && link.view().me().is_some_and(|member| {
                                !member.managed
                                    && member.home.as_ref() == Some(&account.account)
                                    && member.logins.contains(&account.account)
                            }) =>
                    {
                        link.view()
                    }
                    Err(error) => return Err(error),
                };
                view.authorize(&account.account, Action::Work(&view.me)).map_err(Failure::from)
            })();
            match allowed {
                Ok(()) => return Ok(()),
                Err(why) if view_pending(&why) => pending = Some(why),
                Err(why) => refused = Some(why),
            }
        }
        // An account whose hub has not judged yet may yet be let in.
        Err(pending.or(refused).unwrap_or_else(no_subject))
    }

    /// Checks content access before reading either endpoint, including local shortcuts.
    pub(crate) fn authorize_machine(&self, acting: &Acting, machine: &Written) -> Result<(), Failure> {
        if self.is_here(machine) {
            return self.authorize_here(acting);
        }
        self.speakers(acting, machine, Purpose::Hands).map(|_| ()).map_err(|why| Failure::refused(why.to_string()))
    }
}
