//! Conversation deletion outlives the connection that requested it.

use crate::{Node, submitted::Family};
use fastctx_net::link::{Link, Status};
use fastctx_types::{
    AccountName, Address, ResidentId,
    access::{Acting, HubId, Written},
    ipc::Failure,
    owned_records::{Request, Response},
};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};

const FAMILY: Family = Family {
    name: "call/forget-conversation",
    at_once: 8,
    // Stopping a host and deleting its records can take much longer than one call.
    within: Duration::from_secs(30 * 60),
    stays: Duration::from_secs(240),
    // Hub requests are kept before the idempotent local erase, under an after barrier.
    again: true,
};

#[derive(Serialize, Deserialize)]
struct Asked {
    resident: ResidentId,
    machine: Written,
}

#[derive(Serialize, Deserialize)]
enum Destination {
    Here,
    Elsewhere { hub: HubId, account: AccountName, opened: jiff::Timestamp, conversation: Address },
}

// The legacy selector takes the first eligible login. Recheck eligibility of
// the recorded login instead, so a different ordering cannot retarget deletion.
fn legacy_owns(link: &Link, account: &AccountName, conversation: &Address) -> bool {
    let view = link.view();
    if !view.logged_in(&view.me, account) {
        return false;
    }
    let afoot = link.afoot();
    let Some(told) = afoot
        .iter()
        .find(|(name, _)| view.directory.same(name, &conversation.machine))
        .and_then(|(_, told)| told.seen.residents.get(&conversation.resident))
    else {
        return false;
    };
    let works_for = match &told.about.acting {
        Acting::Here => view.member(&conversation.machine).is_some_and(|member| member.logins.contains(account)),
        Acting::Accounts { accounts } => {
            accounts.iter().any(|scope| scope.hub == view.hub && scope.account == *account)
        }
    };
    works_for || view.holds(account, &conversation.machine)
}

pub(super) fn register(node: &Arc<Node>) {
    crate::submitted::register_resolved(
        node,
        FAMILY,
        |node: Arc<Node>, asked: Asked, target: Destination| async move {
            match target {
                Destination::Here => {
                    // Registration follows startup; interrupted deletion cleanup may
                    // still run independently. A missing view is no deletion attempt.
                    if let Some((about, _)) = node.residents.about(&asked.resident) {
                        for link in node.links() {
                            let relevant = match &about.acting {
                                Acting::Here => !node.accounts_on(&link, &about.acting).is_empty(),
                                Acting::Accounts { accounts } => {
                                    accounts.iter().any(|account| account.hub == link.membership().id)
                                }
                            };
                            if relevant {
                                crate::authorization::awaited_view(&link).await.map_err(|failure| {
                                    Failure::not_run(format!("deletion was not started: {}", failure.message))
                                })?;
                            }
                        }
                    }
                    super::ask_here(&node, asked.resident).await
                }
                Destination::Elsewhere { hub, account, opened, conversation } => {
                    let link = node.link_of(&hub).ok_or_else(|| Failure::not_run("the selected hub is unavailable"))?;
                    crate::reach::settled_each(std::slice::from_ref(&link)).await;
                    let view = link.view();
                    if !view.logged_in(&view.me, &account)
                        || view.account(&account).is_none_or(|found| found.opened != opened)
                    {
                        return Err(Failure::refused("this device no longer has the selected account"));
                    }
                    if !matches!(link.status(), Status::Online { .. }) {
                        // Resolution already established this target. A disconnected
                        // ask is kept; its hub judges authority when it is delivered.
                        return super::ask_elsewhere(&node, &link, account, conversation).await;
                    }
                    if view.shared_records {
                        let answer = link
                            .owned_records(&account, Request::Resolve { conversation: conversation.clone() })
                            .await
                            .map_err(Failure::new)?;
                        match answer {
                            Response::Conversation { conversation: current } if current.address == conversation => {}
                            Response::Unregistered if legacy_owns(&link, &account, &conversation) => {}
                            _ => {
                                return Err(Failure::refused(
                                    "the selected account no longer identifies that conversation",
                                ));
                            }
                        }
                    } else if !legacy_owns(&link, &account, &conversation) {
                        return Err(Failure::refused("the selected account no longer owns that conversation"));
                    }
                    super::ask_elsewhere(&node, &link, account, conversation).await
                }
            }
        },
    );
    node.submission_kinds.registered(&FAMILY);
}

async fn direct(node: &Arc<Node>, asked: Asked) -> Result<super::How, Failure> {
    if node.is_here(&asked.machine) {
        return super::ask_here(node, asked.resident).await;
    }
    let link = node
        .locate(&asked.machine)?
        .into_iter()
        .next()
        .ok_or_else(|| Failure::new(format!("no hub this computer is on has a machine named {}", asked.machine)))?;
    let (account, conversation) = link
        .record_actor(&Address { machine: asked.machine.name, resident: asked.resident })
        .await
        .map_err(Failure::new)?;
    super::ask_elsewhere(node, &link, account, conversation).await
}

pub(super) async fn ask(node: &Arc<Node>, resident: ResidentId, machine: Written) -> Result<super::How, Failure> {
    let asked = Asked { resident, machine };
    if !node.submission_kinds.has(&FAMILY) {
        return direct(node, asked).await;
    }
    let elsewhere = !node.is_here(&asked.machine);
    // Only users enter this function. The receipt contains deletion status alone;
    // losing access because the deletion succeeded does not conceal its outcome.
    let result = crate::submitted::submit_resolving(node, &FAMILY, &asked, || async {
        let target = if node.is_here(&asked.machine) {
            super::allowed_here(node, &asked.resident)?;
            Destination::Here
        } else {
            let link = node.locate(&asked.machine)?.into_iter().next().ok_or_else(|| {
                Failure::new(format!("no hub this computer is on has a machine named {}", asked.machine))
            })?;
            // Old hubs can resolve from the cached activity and queue while offline.
            crate::reach::settled_each(std::slice::from_ref(&link)).await;
            let (account, conversation) = link
                .record_actor(&Address { machine: asked.machine.name.clone(), resident: asked.resident.clone() })
                .await
                .map_err(Failure::new)?;
            let view = link.view();
            let opened =
                view.account(&account).ok_or_else(|| Failure::new("the selected account is unavailable"))?.opened;
            Destination::Elsewhere { hub: link.membership().id.clone(), account, opened, conversation }
        };
        // The work must survive deleting the target conversation's own jobs.
        let unit = match &target {
            Destination::Here => format!("forget/here/{}", asked.resident),
            Destination::Elsewhere { hub, conversation, .. } => format!("forget/{hub}/{conversation}"),
        };
        Ok((unit, target))
    })
    .await;
    match result {
        Err(failure) if elsewhere && crate::submission_fallback::unrecorded(&FAMILY, &failure) => {
            direct(node, asked).await
        }
        result => result,
    }
}
