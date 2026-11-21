//! The node's deletion journal retains external cleanup identity after the
//! conversation is fenced. Cleanup never needs to write into a deleted scope.
use super::{operations, records::Exchanged};
use crate::Node;
use fastctx_types::{Address, ResidentId, access::HubAccount, ipc::Failure};
use std::sync::Arc;

pub(crate) async fn prepare(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let scope = id.to_string();
    let recorded = node
        .database
        .read(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM node_conversation_deletions WHERE name=?1)",
                [scope],
                |row| row.get::<_, bool>(0),
            )?)
        })
        .await
        .map_err(failure)?;
    if recorded {
        return Ok(());
    }
    let account = super::scope::pinned(node, id)
        .await?
        .map(|account| serde_json::to_vec(&account))
        .transpose()
        .map_err(failure)?;
    let directory = node.world.conversation(id);
    let fork_source = crate::forks::origin(&directory)
        .map_err(failure)?
        .map(|source| serde_json::to_vec(&source))
        .transpose()
        .map_err(failure)?;
    fastctx_platform::task::spawn_blocking(move || {
        crate::conversation_store::begin_with_account(&directory, account, fork_source)
    })
    .await
    .map_err(failure)?
    .map_err(failure)?;
    crate::content_upkeep::forgetting(node);
    Ok(())
}

pub(crate) async fn apply(node: &Arc<Node>, id: &ResidentId) -> Result<(), Failure> {
    let scope = id.to_string();
    let account = node
        .database
        .read(move |db| {
            db.query_row("SELECT account FROM node_conversation_deletions WHERE name=?1", [scope], |row| {
                row.get::<_, Option<Vec<u8>>>(0)
            })?
            .map(|bytes| crate::records::decode::<HubAccount>(&bytes))
            .transpose()
        })
        .await
        .map_err(failure)?;
    let Some(account) = account else { return Ok(()) };
    let resident = Address { machine: node.machine(), resident: id.clone() };
    match forget(node, id, &account, &resident).await {
        // The hub says there is no such account, or this machine may not act
        // for the one of that name now: none of its records is this
        // machine's to change, and asking again would be refused again.
        Err(failure) if out_of_reach(&failure) => {
            node.log.line(format_args!(
                "the goals of {id} at {}: {}; nothing of them is this machine's to forget there",
                account.account, failure.message
            ));
            node.goals.views.lock().expect("goal views").remove(id);
            crate::sentinels::goal_seams::retire(node, account, resident);
            Ok(())
        }
        done => done,
    }
}

/// Whether the hub's refusal says this machine can do nothing more of an
/// account's records: there is no account of the name, or this machine may
/// not act for it (`fastctx_net::authorization`).
pub(crate) fn out_of_reach(failure: &Failure) -> bool {
    failure.message.contains("there is no account ") || failure.message.contains(" may not act for the account ")
}

/// Forgets at the hub the goals of `id` kept for `account`.
async fn forget(node: &Arc<Node>, id: &ResidentId, account: &HubAccount, resident: &Address) -> Result<(), Failure> {
    let (account, resident) = (account.clone(), resident.clone());
    super::past::forget(node, &account, &resident).await?;
    for _ in 0..3 {
        let (current, _) = operations::by_id(node, id, &account).await?;
        if current.goal.as_ref().is_some_and(|goal| !super::same(node, &goal.resident, &resident)) {
            return Err(Failure::new(super::words::CONVERSATION_CHANGED));
        }
        if current.goal.is_none() {
            super::archive::for_resident(node, &account, id).await?;
            operations::publish(node, id, &current);
            node.goals.views.lock().expect("goal views").remove(id);
            crate::sentinels::goal_seams::retire(node, account, resident);
            return Ok(());
        }
        match operations::bounded(node.goals.records.exchange(
            &account,
            operations::KIND,
            id.as_str(),
            current.version,
            None,
        ))
        .await?
        {
            Exchanged::Written { .. } => {}
            Exchanged::Conflict { .. } => continue,
        }
    }
    Err(Failure::new(super::words::DELETE_CHANGED))
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(error.to_string())
}
