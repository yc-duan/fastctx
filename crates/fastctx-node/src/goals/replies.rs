//! Goal-owned colleagues report conclusions; automatic input does not inherit ordinary reply obligations.

use crate::Node;
use fastctx_types::{
    Address, ResidentId,
    access::HubAccount,
    goals::{Owner, View},
    residents::Origin,
};
use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};

#[derive(Serialize, Deserialize)]
struct Mode {
    account: HubAccount,
    version: u64,
    owner: Option<Address>,
}

fn path(node: &Node, resident: &ResidentId) -> PathBuf {
    node.world.conversation(resident).books().join("goal-replies.json")
}

fn read(node: &Node, resident: &ResidentId) -> io::Result<Option<Mode>> {
    match std::fs::read(path(node, resident)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Record the accepted authority before context installation or other recoverable effects.
/// An older observation cannot undo a newer goal or its explicit clearing.
pub(crate) fn remember(node: &Node, resident: &ResidentId, view: &View) -> io::Result<()> {
    let Some(version) = view.version else { return Ok(()) };
    if !node.residents.about(resident).is_some_and(|(about, _)| matches!(about.origin, Origin::Colleague { .. })) {
        return Ok(());
    }
    let owner = view.goal.as_ref().and_then(|goal| match &goal.owner {
        Owner::Conversation { address } => Some(address.clone()),
        Owner::User => None,
    });
    let _transaction = crate::resident_storage::lock();
    if let Some(prior) = read(node, resident)? {
        if prior.account != view.account {
            return Err(io::Error::other(super::words::ACCOUNT_CHANGED));
        }
        if prior.version >= version {
            return Ok(());
        }
    }
    let path = path(node, resident);
    fastctx_platform::ensure_private_dir(path.parent().expect("goal reply mode directory"))?;
    fastctx_platform::publish(
        &path,
        &serde_json::to_vec(&Mode { account: view.account.clone(), version, owner })?,
        fastctx_platform::Permissions::Private,
    )
}

/// Explicit messages keep their recorded readers. This decision is only for automatic input,
/// before its audience is frozen in a durable handover. A settled goal still owns its report.
pub(crate) fn inherits(node: &Node, resident: &ResidentId) -> io::Result<bool> {
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
        None => match std::fs::read(node.world.conversation(resident).books().join("goal-local.json")) {
            Ok(bytes) => serde_json::from_slice::<View>(&bytes)?.goal.map(|goal| goal.owner),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        },
    };
    Ok(!matches!(owner, Some(Owner::Conversation { address }) if super::same(node, &address, &asked_by)))
}
