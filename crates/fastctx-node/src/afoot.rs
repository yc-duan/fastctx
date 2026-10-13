//! What is afoot on this machine, told to each hub it is on whenever it may
//! have changed (`world-network.md`, 近况): its residents at the grain of
//! turns and the jobs running here, each with the accounts of that hub it
//! works for. A hub hears only of what works for its own accounts; the
//! link tells it only what changed since it last told it.

use std::collections::HashMap;
use std::sync::Arc;

use fastctx_net::link::Link;
use fastctx_types::ResidentId;
use fastctx_types::town::{Afoot, AfootJob, AfootResident, WhoReply};

use crate::Node;

/// Tells the hubs what is afoot here from now on, each time a resident or a
/// job changes, or a hub's view does, until the node stops.
pub(crate) fn start(node: &Arc<Node>) {
    let mut residents = node.residents.changes();
    let mut jobs = node.jobs.subscribe();
    let mut views = node.views.subscribe();
    let node = Arc::downgrade(node);
    tokio::spawn(async move {
        loop {
            {
                let Some(node) = node.upgrade() else { return };
                match crate::server::who_here(&node, true, None, None).await {
                    Ok(reply) => {
                        for link in node.links() {
                            link.tell_afoot(of(&node, &link, &reply));
                        }
                    }
                    Err(failure) => node.log.line(format_args!("reading what is afoot here: {failure}")),
                }
            }
            tokio::select! {
                changed = residents.changed() => if changed.is_err() { return },
                changed = views.changed() => if changed.is_err() { return },
                job = jobs.recv() => match job {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                },
            }
        }
    });
}

/// What `link`'s hub hears of `reply`: the residents and jobs that work for
/// one of its accounts, with those accounts.
fn of(node: &Node, link: &Link, reply: &WhoReply) -> Afoot {
    let mut afoot = Afoot::default();
    let mut working: HashMap<ResidentId, Vec<fastctx_types::AccountName>> = HashMap::new();
    for resident in &reply.residents {
        let accounts = node.accounts_on(link, &resident.about.acting);
        working.insert(resident.about.id.clone(), accounts.clone());
        if !accounts.is_empty() {
            afoot.residents.insert(
                resident.about.id.clone(),
                AfootResident { resident: resident.clone().at_turn_grain(), accounts },
            );
        }
    }
    let hub = &link.membership().id;
    for job in &reply.jobs {
        let accounts = match (&job.account, job.owner.resident()) {
            (Some(account), _) if account.hub == *hub => vec![account.account.clone()],
            (Some(_), _) => Vec::new(),
            (None, Some(owner)) if node.is_me(&owner.machine) => {
                working.get(&owner.resident).cloned().unwrap_or_default()
            }
            (None, _) => Vec::new(),
        };
        if !accounts.is_empty() {
            afoot.jobs.insert(job.job.clone(), AfootJob { job: job.clone(), accounts });
        }
    }
    afoot
}
