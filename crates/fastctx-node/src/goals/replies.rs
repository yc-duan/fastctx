//! Goal-owned colleagues report conclusions; automatic input does not inherit ordinary reply obligations.

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::HubAccount,
    goals::{Owner, View},
    residents::Origin,
};
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Serialize, Deserialize)]
struct Mode {
    account: HubAccount,
    version: u64,
    owner: Option<Address>,
}

fn read(node: &Node, resident: &ResidentId) -> io::Result<Option<Mode>> {
    let directory = node.world.conversation(resident);
    initialize(&directory)?;
    crate::records::read(&directory, |rows| rows.get("goal-replies", "current"))
}
pub(crate) fn initialize(directory: &fastctx_types::layout::ConversationDir) -> io::Result<()> {
    crate::records::legacy::single::<Mode>(directory, "goal-replies", &directory.books().join("goal-replies.json"))
}

/// Record the accepted authority before context installation or other recoverable effects.
/// An older observation cannot undo a newer goal or its explicit clearing.
fn remember_now(node: &Node, resident: &ResidentId, view: &View) -> io::Result<()> {
    let Some(version) = view.version else { return Ok(()) };
    if !node.residents.about(resident).is_some_and(|(about, _)| matches!(about.origin, Origin::Colleague { .. })) {
        return Ok(());
    }
    let owner = view.goal.as_ref().and_then(|goal| match &goal.owner {
        Owner::Conversation { address } => Some(address.clone()),
        Owner::User => None,
    });
    let directory = node.world.conversation(resident);
    initialize(&directory)?;
    let mode = Mode { account: view.account.clone(), version, owner };
    crate::records::write_at(&directory, fastctx_storage::Priority::Interactive, 512, move |rows| {
        if let Some(prior) = rows.get::<Mode>("goal-replies", "current")? {
            if prior.account != mode.account {
                return Err(fastctx_storage::Error::new(super::words::ACCOUNT_CHANGED));
            }
            if prior.version >= mode.version {
                return Ok(());
            }
        }
        rows.put("goal-replies", "current", &mode)
    })
}

/// Explicit messages keep their recorded readers. This decision is only for automatic input,
/// before its audience is frozen in a durable handover. A settled goal still owns its report.
fn inherits_now(node: &Node, resident: &ResidentId) -> io::Result<bool> {
    let Some((about, _)) = node.residents.about(resident) else { return Ok(true) };
    let Origin::Colleague { asked_by, .. } = about.origin else { return Ok(true) };
    if let Some(mode) = read(node, resident)? {
        return Ok(!mode.owner.is_some_and(|owner| super::same(node, &owner, &asked_by)));
    }
    // Earlier local goals predate the separate accepted reply mode.
    let cached = node
        .goals
        .views
        .lock()
        .expect("goal views")
        .get(resident)
        .map(|view| view.goal.as_ref().map(|goal| goal.owner.clone()));
    let owner = match cached {
        Some(owner) => owner,
        None => {
            super::local::get(&node.world.conversation(resident))?.and_then(|view| view.goal.map(|goal| goal.owner))
        }
    };
    Ok(!matches!(owner, Some(Owner::Conversation { address }) if super::same(node, &address, &asked_by)))
}

pub(crate) async fn remember(node: &std::sync::Arc<Node>, resident: &ResidentId, view: &View) -> io::Result<()> {
    let (node, resident, view) = (node.clone(), resident.clone(), view.clone());
    tokio::task::spawn_blocking(move || remember_now(&node, &resident, &view)).await.map_err(io::Error::other)?
}
pub(crate) async fn inherits(node: &std::sync::Arc<Node>, resident: &ResidentId) -> io::Result<bool> {
    let (node, resident) = (node.clone(), resident.clone());
    tokio::task::spawn_blocking(move || inherits_now(&node, &resident)).await.map_err(io::Error::other)?
}
