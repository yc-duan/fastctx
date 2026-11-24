//! The colleagues a conversation asked go with it (the user, 2026-11-23:
//! 「删除一段会话，那这段会话他派发的同事也要一并删除…站在各台机器上的副本，
//! 它的一切资产，包括图片啥的也要删除…不然这些派发的同事会话，他根本就没有
//! 地方可以删」). Before a conversation that lives here goes, each colleague
//! it asked goes too, as the user deleting it would have it go, with what
//! every machine and device keeps of it: one that lives here, here and now,
//! with the colleagues it asked in turn; one that lives elsewhere, by the hub
//! it was asked through, which has its machine delete it, asked only once the
//! conversation is gone here (`Ask::ready`), so that a deletion refused takes
//! none of those with it. A colleague here that cannot be deleted keeps the
//! conversation from going this time, as a command of its own would.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use fastctx_types::access::{Acting, HubAccount};
use fastctx_types::ipc::Failure;
use fastctx_types::residents::Origin;
use fastctx_types::{Address, ResidentId};

use super::Ask;
use crate::Node;

/// How the conversation goes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Going {
    /// Someone asked: what its colleagues are doing is stopped first.
    Asked,
    /// Its retention ended: refused while a colleague of it here is at work.
    Expired,
}

/// What its colleagues left to ask of the hubs.
#[derive(Default)]
pub(super) struct Went {
    /// The asks kept now for its colleagues elsewhere, held back until it is
    /// gone here: taken back where it does not go after all.
    pub(super) added: Vec<Ask>,
    /// Whether anything is kept to be asked of a hub, to be sent once it is
    /// gone here.
    pub(super) asking: bool,
}

/// Deletes the colleagues `id`, which lives here, asked, before it goes.
pub(super) async fn go(node: &Arc<Node>, id: &ResidentId, going: Going) -> Result<Went, String> {
    let acting = node.residents.about(id).map(|(about, _)| about.acting);
    let mut went = Went::default();
    let mut asks = Vec::new();
    for (colleague, account) in asked(node, id).await {
        if node.is_me(&colleague.machine) {
            went.asking |= here(node, id, &colleague.resident, going).await?;
            continue;
        }
        let asking = elsewhere(node, id, &colleague, account, acting.as_ref());
        if asking.is_empty() {
            node.log.line(format_args!(
                "{} on {}, a colleague {id} asked, is asked of no hub to be deleted: none this computer is on has its machine",
                colleague.resident, colleague.machine
            ));
        }
        asks.extend(asking);
    }
    if !asks.is_empty() {
        went.added = super::keep(node, asks)
            .await
            .map_err(|error| format!("keeping what to ask the hubs of its colleagues: {error}"))?;
        went.asking = true;
    }
    Ok(went)
}

/// The colleagues `id` asked, each with the account it was asked for where
/// one is recorded: those its actions recorded, and those living here that
/// name it as their asker though its actions do not. What cannot be read of
/// its actions is told in the log and leaves those to the second way: the
/// conversation goes all the same. Of one whose folder is gone, its actions
/// are not read: reading them would record them anew.
async fn asked(node: &Arc<Node>, id: &ResidentId) -> BTreeMap<Address, Option<HubAccount>> {
    let mut found = BTreeMap::new();
    let folder = node.world.conversation(id).path().to_path_buf();
    let path = node.world.conversation(id).actions();
    let reading = move || -> std::io::Result<Vec<fastctx_types::residents::Action>> {
        if !fastctx_platform::disk::exists(&folder)? {
            return Ok(Vec::new());
        }
        crate::residents::activity::asked(&path, None)
    };
    let actions = match fastctx_platform::task::spawn_blocking(reading).await {
        Ok(Ok(actions)) => actions,
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Ok(Err(error)) => {
            node.log.line(format_args!("reading the colleagues {id} asked: {error}"));
            Vec::new()
        }
        Err(error) => {
            node.log.line(format_args!("reading the colleagues {id} asked: {error}"));
            Vec::new()
        }
    };
    for action in actions.into_iter().filter(|action| action.hand == "ask_colleague") {
        let Ok(resident) = action.result.parse::<ResidentId>() else { continue };
        let recorded = Address { machine: action.machine, resident };
        let account = match crate::collab::asked_account(node, id, &recorded).await {
            Ok(account) => account,
            Err(failure) => {
                node.log.line(format_args!(
                    "reading the account {id} asked {} for: {}",
                    recorded.resident, failure.message
                ));
                None
            }
        };
        let address = if node.is_me(&recorded.machine) {
            Address { machine: node.machine(), resident: recorded.resident }
        } else {
            recorded
        };
        found.entry(address).or_insert(account);
    }
    for resident in node.residents.everyone() {
        if let Origin::Colleague { asked_by, .. } = &resident.about.origin
            && asked_by.resident == *id
            && node.is_me(&asked_by.machine)
        {
            found.entry(Address { machine: node.machine(), resident: resident.id }).or_insert(None);
        }
    }
    found
}

/// Deletes `id`, a colleague `asker` asked that lives here, as the user
/// deleting it would: says whether it kept what to ask the hubs. One gone
/// already is passed over, and so is one that does not name `asker` as its
/// asker: an action alone does not make another conversation a colleague.
async fn here(node: &Arc<Node>, asker: &ResidentId, id: &ResidentId, going: Going) -> Result<bool, String> {
    if node.residents.ever_deleted(id) {
        return Ok(false);
    }
    if let Some((about, _)) = node.residents.about(id) {
        let named = matches!(
            &about.origin,
            Origin::Colleague { asked_by, .. } if asked_by.resident == *asker && node.is_me(&asked_by.machine)
        );
        if !named {
            node.log.line(format_args!("{id}, which the actions of {asker} name, was not asked by it: kept"));
            return Ok(false);
        }
        if going == Going::Expired && crate::conversation_retention::expiry::busy(node, id).await {
            return Err(format!("its colleague {id} is at work"));
        }
    }
    let asking = deleting(node, id.clone())
        .await
        .map_err(|failure| format!("deleting its colleague {id}: {}", failure.message))?;
    node.log.line(format_args!("deleted {id}, which lived here, with {asker}, the conversation that asked it"));
    Ok(asking)
}

/// `super::delete_here`, its future boxed: deleting a colleague deletes the
/// colleagues it asked in turn.
fn deleting(node: &Arc<Node>, id: ResidentId) -> Pin<Box<dyn Future<Output = Result<bool, Failure>> + Send + '_>> {
    Box::pin(async move { super::delete_here(node, &id).await })
}

/// What to ask of the hubs for `colleague`, which lives elsewhere: of the hub
/// it was asked through, as the account it was asked for; for one asked before
/// that was recorded, of each hub its machine is on, as each account the
/// conversation that asked it works for there. Each is held back until
/// `asker` is gone here.
fn elsewhere(
    node: &Node,
    asker: &ResidentId,
    colleague: &Address,
    account: Option<HubAccount>,
    acting: Option<&Acting>,
) -> Vec<Ask> {
    let ask = |hub, account| Ask {
        hub,
        account,
        conversation: colleague.clone(),
        after: Some(asker.clone()),
        round: None,
        since: None,
    };
    if let Some(HubAccount { hub, account }) = account {
        return vec![ask(hub, account)];
    }
    let Some(acting) = acting else { return Vec::new() };
    node.links()
        .iter()
        .filter(|link| link.view().member(&colleague.machine).is_some())
        .flat_map(|link| {
            let hub = link.membership().id.clone();
            node.accounts_on(link, acting).into_iter().map(move |account| (hub.clone(), account))
        })
        .map(|(hub, account)| ask(hub, account))
        .collect()
}
