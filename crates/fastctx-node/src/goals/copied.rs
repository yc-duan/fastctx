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
use fastctx_types::access::HubAccount;
use fastctx_types::goals::{Goal, GoalPast, Request, Updated, View};
use fastctx_types::{Address, MachineName, ResidentId};
use std::sync::Arc;

/// `request`, a read of the goal of the conversation `resident` on
/// `machine`, as the copy here holds it; none when it is not a read the
/// copy answers, both the machine and the hub are reached, or no such copy
/// is here.
pub(crate) async fn answer(
    node: &Arc<Node>,
    resident: &ResidentId,
    machine: &MachineName,
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
    machine: &MachineName,
    account: Option<&HubAccount>,
    request: &Request,
) -> Option<Updated> {
    let library = &node.assets.as_ref()?.library;
    let scope = copy_of(node, resident, Some(machine))?;
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
            if goal.resident.resident != *resident
                || goal.resident.machine != *machine
                || account.is_some_and(|account| goal.account != *account)
            {
                return None;
            }
            Some((record.metadata.version, goal))
        }
        None => None,
    };
    match request {
        Request::Read => {
            let account = account.cloned().or_else(|| goal.as_ref().map(|(_, goal)| goal.account.clone()))?;
            let (version, goal) = goal.map_or((None, None), |(version, goal)| (Some(version), Some(goal)));
            Some(Updated::Goal { view: Box::new(View { account, version, goal, pending: None }) })
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
