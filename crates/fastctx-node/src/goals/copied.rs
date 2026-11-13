//! A conversation's goal and its earlier goals as the copy here holds them,
//! read while the machine it lives on, or its hub, is not reached (plan
//! 3-5, design §28.27): the hub orders the account records `goal` and
//! `goal_past` into the conversation's scope (`goals` stream, `hub
//! assets::mirror`). What the copy does not hold is not answered from it: a
//! goal whose long text went to records of its own (`goal_text`), and the
//! goal's history pages. The hold the machine it lives on keeps is not in
//! it either, so a stop that machine has not written yet shows as before.

use crate::Node;
use crate::timeline::copied_records::{copy_of, record_in};
use fastctx_content::v2::{ConversationGoal, Kind as _};
use fastctx_types::access::{Acting, HubAccount, Written};
use fastctx_types::goals::{Goal, GoalPast, Request, Updated, View};
use fastctx_types::{Address, ResidentId};
use std::sync::Arc;

/// `request`, a read of the goal of the conversation `resident` on
/// `machine` (of the hub it names, where it names one), asked by `account`
/// where given, as the copy here holds it; none when it is not a read the
/// copy answers, both the machine and the hub are reached, `account` is not
/// logged in here, or no such copy is here.
pub(crate) async fn answer(
    node: &Arc<Node>,
    resident: &ResidentId,
    machine: &Written,
    account: Option<&HubAccount>,
    request: &Request,
) -> Option<Updated> {
    if !matches!(request, Request::Read | Request::Past) {
        return None;
    }
    let (node, resident, machine, account, request) =
        (node.clone(), resident.clone(), machine.clone(), account.cloned(), request.clone());
    fastctx_platform::task::spawn_blocking(move || answered(&node, &resident, &machine, account.as_ref(), &request))
        .await
        .ok()
        .flatten()
}

fn answered(
    node: &Node,
    resident: &ResidentId,
    written: &Written,
    account: Option<&HubAccount>,
    request: &Request,
) -> Option<Updated> {
    let library = &node.assets.as_ref()?.library;
    // Asked as an account: one logged in here, and the copy that account's
    // network holds; otherwise the network the machine is named on, if any.
    if let Some(account) = account
        && !node.serves(&Acting::Here, &account.hub, &account.account)
    {
        return None;
    }
    // The network named, as the account's and the machine's alike: two
    // that differ are not answered from here (codex-847f57fd #14).
    let named = match &written.hub {
        Some(hub) => Some(crate::timeline::copy::hub_id_of(node, hub)?),
        None => None,
    };
    let hub = match (account, named) {
        (Some(account), Some(named)) if account.hub.to_string() != named => return None,
        (Some(account), _) => Some(account.hub.to_string()),
        (None, named) => named,
    };
    let machine = &written.name;
    let scope = copy_of(node, resident, Some(machine), hub.as_deref(), account.map(|account| &account.account))?;
    let machine_reached = node.is_me(machine) || !crate::timeline::copy::away(node, machine);
    if machine_reached && !crate::timeline::copy::hub_away(node, &scope) {
        return None;
    }
    let record = |kind: &str| {
        record_in(library, &scope.key, ConversationGoal::STREAM, kind).map(|record| {
            record.filter(|record| !record.metadata.deleted && record.metadata.kind == kind && record.metadata.key == resident.as_str())
        })
    };
    let goal = match record(super::operations::KIND)? {
        Some(record) => {
            // Its long text in records of their own, which no copy holds.
            if record.body.get("_goal_storage").is_some() {
                return None;
            }
            let goal: Goal = serde_json::from_value(record.body).ok()?;
            if goal.resident.resident != *resident || goal.resident.machine != *machine {
                return None;
            }
            Some((record.metadata.version, goal))
        }
        None => None,
    };
    match request {
        Request::Read => {
            // Whose goal it is, as the hub's answer says: the account it is
            // kept under, the one that opened the conversation.
            let owner = match &goal {
                Some((_, goal)) => goal.account.clone(),
                None => opener(&scope)?,
            };
            let (version, goal) = goal.map_or((None, None), |(version, goal)| (Some(version), Some(goal)));
            Some(Updated::Goal { view: Box::new(View { account: owner, version, goal, pending: None }) })
        }
        Request::Past => {
            let owner = Address { machine: machine.clone(), resident: resident.clone() };
            let mut past = record(super::past::KIND)?
                .and_then(|record| serde_json::from_value::<GoalPast>(record.body).ok())
                .filter(|past| past.resident.as_ref().is_none_or(|whose| *whose == owner))
                .unwrap_or_default();
            // The current goal is never among the earlier ones.
            if let Some((_, goal)) = &goal {
                past.goals.retain(|kept| kept.id != goal.id);
            }
            Some(Updated::Past { past })
        }
        _ => None,
    }
}

/// The one account that opened the conversation whose copy `scope` is, as
/// the hub described it; none where it names none, or several.
fn opener(scope: &fastctx_content::v2::HeldScope) -> Option<HubAccount> {
    let descriptor: fastctx_types::assets::Descriptor = serde_json::from_slice(scope.description.as_deref()?).ok()?;
    let opener = match &descriptor.readers {
        fastctx_types::assets::Readers::Conversation { openers, .. } => match openers.as_slice() {
            [opener] => opener,
            _ => return None,
        },
        // A body's conversation, its opener as the body was kept
        // (codex-847f57fd #15).
        fastctx_types::assets::Readers::Body { opener, .. } => opener,
        _ => return None,
    };
    Some(HubAccount { hub: scope.hub.clone()?.try_into().ok()?, account: opener.clone() })
}
